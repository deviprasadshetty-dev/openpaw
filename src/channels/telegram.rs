use crate::channels::root::{Channel, ParsedMessage};
use crate::config_types::TelegramConfig;
use crate::interactions::choices::parse_assistant_choices;
use anyhow::{Result, anyhow};
use base64::Engine as _;
use regex::Regex;
use reqwest::blocking::{Client, multipart};
use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::warn;
use uuid::Uuid;

const MAX_MESSAGE_LEN: usize = 4096;
const MAX_INLINE_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const API_BASE: &str = "https://api.telegram.org/bot";
/// Temp dir for inbound files downloaded from Telegram
const INBOUND_TEMP_DIR: &str = "openpaw-tg-inbound";

// ── Attachment kinds ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachmentKind {
    Document,
    Image,
    Audio,
}

#[derive(Debug, Clone)]
struct OutboundAttachment {
    kind: AttachmentKind,
    path: PathBuf,
}

// ── TelegramChannel ─────────────────────────────────────────────────────────

pub struct TelegramChannel {
    config: TelegramConfig,
    /// reqwest blocking client — used for all Telegram API calls.
    client: Client,
    /// Teloxide getUpdates offset (next update_id to request).
    last_update_id: AtomicI64,
    /// Maps chat_id -> message_id of the in-progress streaming message.
    active_streams: Mutex<HashMap<String, i64>>,
    /// Maps chat_id -> stop flag for the typing heartbeat thread.
    typing_stops: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl TelegramChannel {
    pub fn new(config: TelegramConfig) -> Self {
        Self {
            client: Client::new(),
            config,
            last_update_id: AtomicI64::new(0),
            active_streams: Mutex::new(HashMap::new()),
            typing_stops: Mutex::new(HashMap::new()),
        }
    }

    // ── polling offset helpers ──────────────────────────────────────────────

    fn api_url(&self, method: &str) -> String {
        format!("{}{}/{}", API_BASE, self.config.bot_token, method)
    }

    pub fn set_initial_update_offset(&self, offset: i64) {
        if offset <= 0 {
            return;
        }
        let current = self.last_update_id.load(Ordering::Relaxed);
        if offset > current {
            self.last_update_id.store(offset, Ordering::Relaxed);
        }
    }

    pub fn current_update_offset(&self) -> i64 {
        self.last_update_id.load(Ordering::Relaxed)
    }

    // ── auth helpers ────────────────────────────────────────────────────────

    fn is_user_allowed(&self, username: &str, user_id: &str) -> bool {
        if self.config.allow_from.is_empty() {
            return false;
        }
        for allowed in &self.config.allow_from {
            if allowed == "*" {
                return true;
            }
            let check = if allowed.starts_with('@') {
                &allowed[1..]
            } else {
                allowed.as_str()
            };
            if check.eq_ignore_ascii_case(username) || check == user_id {
                return true;
            }
        }
        false
    }

    fn is_group_user_allowed(&self, username: &str, user_id: &str) -> bool {
        match self.config.group_policy.as_str() {
            "open" => return true,
            "disabled" => return false,
            _ => {}
        }
        let check_list = if !self.config.group_allow_from.is_empty() {
            &self.config.group_allow_from
        } else {
            &self.config.allow_from
        };
        if check_list.is_empty() {
            return false;
        }
        for allowed in check_list {
            if allowed == "*" {
                return true;
            }
            let check = if allowed.starts_with('@') {
                &allowed[1..]
            } else {
                allowed.as_str()
            };
            if check.eq_ignore_ascii_case(username) || check == user_id {
                return true;
            }
        }
        false
    }

    fn is_authorized(&self, is_group: bool, username: &str, user_id: &str) -> bool {
        if is_group {
            self.is_group_user_allowed(username, user_id)
        } else {
            self.is_user_allowed(username, user_id)
        }
    }

    // ── typing indicator ────────────────────────────────────────────────────

    /// Fire a single sendChatAction=typing (best-effort).
    fn send_typing_once(&self, chat_id: &str) {
        let url = self.api_url("sendChatAction");
        let body = serde_json::json!({ "chat_id": chat_id, "action": "typing" });
        let _ = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send();
    }

    /// Spawn a background thread that keeps sending typing every 4 s until the
    /// returned `Arc<AtomicBool>` stop flag is set to `true`.
    fn start_typing_heartbeat(&self, chat_id: &str) -> Arc<AtomicBool> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let url = self.api_url("sendChatAction");
        let client = self.client.clone();
        let chat_id_owned = chat_id.to_string();

        std::thread::spawn(move || {
            let body = serde_json::json!({ "chat_id": chat_id_owned, "action": "typing" });
            while !stop_clone.load(Ordering::Relaxed) {
                let _ = client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send();
                std::thread::sleep(Duration::from_secs(4));
            }
        });

        stop
    }

    /// Stop the typing heartbeat for a chat (if one is running).
    fn stop_typing_heartbeat(&self, chat_id: &str) {
        if let Ok(mut map) = self.typing_stops.lock() {
            if let Some(flag) = map.remove(chat_id) {
                flag.store(true, Ordering::Relaxed);
            }
        }
    }

    // ── inbound file download ───────────────────────────────────────────────

    /// Download a Telegram file by file_id to a local temp path.
    /// Returns the local path on success.
    fn download_tg_file(&self, file_id: &str, prefix: &str, ext: &str) -> Option<PathBuf> {
        // 1. getFile → file_path
        let url = self.api_url("getFile");
        let body = serde_json::json!({ "file_id": file_id });
        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .ok()?;
        let resp_val: Value = serde_json::from_str(&resp.text().ok()?).ok()?;
        let file_path = resp_val
            .get("result")?
            .get("file_path")?
            .as_str()?
            .to_string();

        // 2. Download bytes
        let dl_url = format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.config.bot_token, file_path
        );
        let bytes = self.client.get(&dl_url).send().ok()?.bytes().ok()?;
        if bytes.is_empty() {
            return None;
        }

        // 3. Write to temp dir
        let dir = std::env::temp_dir().join(INBOUND_TEMP_DIR);
        std::fs::create_dir_all(&dir).ok()?;
        let filename = format!("{}-{}.{}", prefix, Uuid::new_v4(), ext);
        let dest = dir.join(&filename);
        std::fs::write(&dest, &bytes).ok()?;
        Some(dest)
    }

    /// Extract content from a message. For media messages, download the file
    /// and return a formatted description with the local path so the agent can
    /// read it.
    fn extract_message_content(&self, message: &Value) -> String {
        // Text
        if let Some(text) = message.get("text").and_then(|v| v.as_str()) {
            return text.to_string();
        }

        let caption = message
            .get("caption")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Photo — pick highest-res photo (last in array)
        if let Some(photos) = message.get("photo").and_then(|v| v.as_array()) {
            if let Some(last_photo) = photos.last() {
                let file_id = last_photo
                    .get("file_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(path) = self.download_tg_file(file_id, "photo", "jpg") {
                    let cap = if caption.is_empty() {
                        String::new()
                    } else {
                        format!("\nCaption: {}", caption)
                    };
                    return format!("[User sent photo: {}{}]", path.display(), cap);
                }
                return format!(
                    "[Photo: file_id={}]{}",
                    file_id,
                    if caption.is_empty() {
                        String::new()
                    } else {
                        format!(" Caption: {}", caption)
                    }
                );
            }
        }

        // Document
        if let Some(doc) = message.get("document") {
            let file_id = doc.get("file_id").and_then(|v| v.as_str()).unwrap_or("");
            let file_name = doc
                .get("file_name")
                .and_then(|v| v.as_str())
                .unwrap_or("document");
            let ext = Path::new(file_name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("bin");
            if let Some(path) = self.download_tg_file(file_id, "doc", ext) {
                let cap = if caption.is_empty() {
                    String::new()
                } else {
                    format!("\nCaption: {}", caption)
                };
                return format!(
                    "[User sent document '{}': {}{}]",
                    file_name,
                    path.display(),
                    cap
                );
            }
            return format!("[Document: {} file_id={}]", file_name, file_id);
        }

        // Voice
        if let Some(voice) = message.get("voice") {
            let file_id = voice.get("file_id").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(path) = self.download_tg_file(file_id, "voice", "ogg") {
                return format!("[User sent voice message: {}]", path.display());
            }
            return format!("[Voice message: file_id={}]", file_id);
        }

        // Audio
        if let Some(audio) = message.get("audio") {
            let file_id = audio.get("file_id").and_then(|v| v.as_str()).unwrap_or("");
            let title = audio
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("audio");
            if let Some(path) = self.download_tg_file(file_id, "audio", "mp3") {
                return format!("[User sent audio '{}': {}]", title, path.display());
            }
            return format!("[Audio: {} file_id={}]", title, file_id);
        }

        // Video
        if let Some(video) = message.get("video") {
            let file_id = video.get("file_id").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(path) = self.download_tg_file(file_id, "video", "mp4") {
                let cap = if caption.is_empty() {
                    String::new()
                } else {
                    format!("\nCaption: {}", caption)
                };
                return format!("[User sent video: {}{}]", path.display(), cap);
            }
            return format!("[Video: file_id={}]", file_id);
        }

        // Caption-only (e.g. sticker with caption)
        if !caption.is_empty() {
            return caption;
        }

        String::new()
    }

    // ── update processing ───────────────────────────────────────────────────

    fn process_update(&self, update: &Value, messages: &mut Vec<ParsedMessage>) {
        // Track offset
        if let Some(uid) = update.get("update_id").and_then(|v| v.as_i64()) {
            let current = self.last_update_id.load(Ordering::Relaxed);
            if uid >= current {
                self.last_update_id.store(uid + 1, Ordering::Relaxed);
            }
        }
        let update_id = update.get("update_id").and_then(|v| v.as_i64());

        // Callback query
        if let Some(cbq) = update.get("callback_query") {
            self.process_callback_query(cbq, update_id, messages);
            return;
        }

        let message = match update.get("message") {
            Some(m) => m,
            None => return,
        };

        let from_obj = match message.get("from").and_then(|v| v.as_object()) {
            Some(f) => f,
            None => return,
        };

        let username = from_obj
            .get("username")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let user_id = from_obj
            .get("id")
            .and_then(|v| v.as_i64())
            .map(|id| id.to_string())
            .unwrap_or_default();

        let chat_obj = match message.get("chat").and_then(|v| v.as_object()) {
            Some(c) => c,
            None => return,
        };
        let chat_id = chat_obj
            .get("id")
            .and_then(|v| v.as_i64())
            .map(|id| id.to_string())
            .unwrap_or_default();
        let is_group = chat_obj
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t != "private")
            .unwrap_or(false);

        if !self.is_authorized(is_group, username, &user_id) {
            warn!(
                "ignoring message from unauthorized user: username={}, user_id={}",
                username, user_id
            );
            return;
        }

        let first_name = from_obj
            .get("first_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let message_id = message.get("message_id").and_then(|v| v.as_i64());
        let content = self.extract_message_content(message);

        if content.is_empty() {
            return;
        }

        let sender_identity = if username != "unknown" {
            username.to_string()
        } else {
            user_id.clone()
        };

        messages.push(ParsedMessage {
            sender_id: sender_identity,
            chat_id: format!("telegram:{}", chat_id),
            content,
            session_key: format!("telegram:{}", chat_id),
            is_group,
            update_id,
            message_id,
            username: if username != "unknown" {
                Some(username.to_string())
            } else {
                None
            },
            first_name,
        });

        // ── Note: chat_id in ParsedMessage.chat_id is "telegram:<id>" ──
        // For the raw numeric chat_id we re-derive it from session_key downstream.
        // Actually we must store the raw numeric chat_id in ParsedMessage.chat_id
        // for send_message to work — restore that below:
    }

    fn process_callback_query(
        &self,
        cbq: &Value,
        update_id: Option<i64>,
        messages: &mut Vec<ParsedMessage>,
    ) {
        let cb_id = cbq.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let cb_data = cbq.get("data").and_then(|v| v.as_str()).unwrap_or("");
        self.answer_callback_query(cb_id, None);

        let from_obj = match cbq.get("from").and_then(|v| v.as_object()) {
            Some(f) => f,
            None => return,
        };
        let username = from_obj
            .get("username")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let user_id = from_obj
            .get("id")
            .and_then(|v| v.as_i64())
            .map(|id| id.to_string())
            .unwrap_or_default();

        let message_obj = match cbq.get("message").and_then(|v| v.as_object()) {
            Some(m) => m,
            None => return,
        };
        let chat_obj = match message_obj.get("chat").and_then(|v| v.as_object()) {
            Some(c) => c,
            None => return,
        };
        let chat_id = chat_obj
            .get("id")
            .and_then(|v| v.as_i64())
            .map(|id| id.to_string())
            .unwrap_or_default();
        let is_group = chat_obj
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t != "private")
            .unwrap_or(false);

        if !self.is_authorized(is_group, username, &user_id) {
            warn!(
                "ignoring callback from unauthorized user: username={}, user_id={}",
                username, user_id
            );
            self.answer_callback_query(cb_id, Some("You are not authorized to use this button"));
            return;
        }

        let sender_identity = if username != "unknown" {
            username.to_string()
        } else {
            user_id.clone()
        };

        messages.push(ParsedMessage {
            sender_id: sender_identity,
            chat_id: chat_id.clone(),
            content: format!("[Callback: {}]", cb_data),
            session_key: format!("telegram:{}", chat_id),
            is_group,
            update_id,
            message_id: message_obj.get("message_id").and_then(|v| v.as_i64()),
            username: if username != "unknown" {
                Some(username.to_string())
            } else {
                None
            },
            first_name: from_obj
                .get("first_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        });
    }

    fn answer_callback_query(&self, cb_id: &str, text: Option<&str>) {
        let url = self.api_url("answerCallbackQuery");
        let mut body = serde_json::json!({ "callback_query_id": cb_id });
        if let Some(t) = text {
            body["text"] = Value::String(t.to_string());
        }
        let _ = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send();
    }

    // ── outbound text ───────────────────────────────────────────────────────

    fn send_message_with_splitting(
        &self,
        chat_id: &str,
        text: &str,
        reply_to: Option<i64>,
    ) -> Result<()> {
        if text.len() <= MAX_MESSAGE_LEN {
            return self.send_single_message(chat_id, text, reply_to);
        }
        let chunks = self.smart_split(text, MAX_MESSAGE_LEN - 12);
        for (i, chunk) in chunks.iter().enumerate() {
            let is_last = i == chunks.len() - 1;
            let to_send = if is_last {
                chunk.to_string()
            } else {
                format!("{}\n\n⏬", chunk)
            };
            self.send_single_message(chat_id, &to_send, if i == 0 { reply_to } else { None })?;
        }
        Ok(())
    }

    fn smart_split(&self, text: &str, max_len: usize) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut remaining = text;
        while !remaining.is_empty() {
            if remaining.len() <= max_len {
                chunks.push(remaining.to_string());
                break;
            }
            let search_area = &remaining[..max_len];
            let half = max_len / 2;
            let split_at = if let Some(pos) = search_area.rfind('\n') {
                if pos >= half {
                    pos + 1
                } else if let Some(space_pos) = search_area.rfind(' ') {
                    space_pos + 1
                } else {
                    max_len
                }
            } else if let Some(pos) = search_area.rfind(' ') {
                pos + 1
            } else {
                max_len
            };
            chunks.push(remaining[..split_at].to_string());
            remaining = &remaining[split_at..];
        }
        chunks
    }

    fn markdown_to_html(&self, text: &str) -> String {
        use pulldown_cmark::{Event, Parser, Tag, TagEnd};

        fn html_escape(s: &str) -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        }

        let parser = Parser::new(text);
        let mut html = String::new();
        for event in parser {
            match event {
                Event::Start(Tag::Strong) => html.push_str("<b>"),
                Event::End(TagEnd::Strong) => html.push_str("</b>"),
                Event::Start(Tag::Emphasis) => html.push_str("<i>"),
                Event::End(TagEnd::Emphasis) => html.push_str("</i>"),
                Event::Start(Tag::CodeBlock(_)) => html.push_str("<pre><code>"),
                Event::End(TagEnd::CodeBlock) => html.push_str("</code></pre>"),
                Event::Start(Tag::Link { dest_url, .. }) => {
                    html.push_str(&format!("<a href=\"{}\">", dest_url))
                }
                Event::End(TagEnd::Link) => html.push_str("</a>"),
                Event::Start(Tag::Strikethrough) => html.push_str("<s>"),
                Event::End(TagEnd::Strikethrough) => html.push_str("</s>"),
                Event::Start(Tag::Item) => html.push_str("• "),
                Event::End(TagEnd::Item) => html.push('\n'),
                Event::Start(Tag::Paragraph) => {}
                Event::End(TagEnd::Paragraph) => html.push_str("\n\n"),
                Event::Code(c) => html.push_str(&format!("<code>{}</code>", html_escape(&c))),
                Event::Text(t) => html.push_str(&html_escape(&t)),
                Event::SoftBreak => html.push('\n'),
                Event::HardBreak => html.push('\n'),
                _ => {}
            }
        }
        // Trim trailing newlines and spaces that might be left by paragraph ends
        html.trim_end().to_string()
    }

    fn parse_outbound_attachments(&self, text: &str) -> (String, Vec<OutboundAttachment>) {
        // FILE / DOCUMENT / IMAGE / AUDIO markers
        let marker_re = Regex::new(r"(?i)\[(FILE|DOCUMENT|IMAGE|AUDIO):([^\]]+)\]").unwrap();

        let mut attachments = Vec::new();
        for caps in marker_re.captures_iter(text) {
            let kind = match caps
                .get(1)
                .map(|m| m.as_str().to_ascii_uppercase())
                .as_deref()
            {
                Some("IMAGE") => AttachmentKind::Image,
                Some("AUDIO") => AttachmentKind::Audio,
                Some(_) => AttachmentKind::Document,
                None => continue,
            };

            let mut raw_path = caps
                .get(2)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();

            if (raw_path.starts_with('"') && raw_path.ends_with('"'))
                || (raw_path.starts_with('\'') && raw_path.ends_with('\''))
            {
                raw_path = raw_path[1..raw_path.len().saturating_sub(1)].to_string();
            }

            if raw_path.is_empty() {
                continue;
            }

            if let Some(path) = self.resolve_attachment_path(&raw_path) {
                attachments.push(OutboundAttachment { kind, path });
            } else {
                warn!("Attachment marker path not found: {}", raw_path);
            }
        }

        let markerless = marker_re.replace_all(text, "").to_string();
        let (without_inline, inline_attachments) =
            self.extract_inline_data_uri_attachments(&markerless);
        attachments.extend(inline_attachments);

        let empty_lines_re = Regex::new(r"\n{3,}").unwrap();
        let cleaned = empty_lines_re
            .replace_all(without_inline.trim(), "\n\n")
            .to_string();

        (cleaned, attachments)
    }

    fn extract_inline_data_uri_attachments(&self, text: &str) -> (String, Vec<OutboundAttachment>) {
        let mut attachments = Vec::new();
        let mut kept_lines = Vec::new();

        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(data_uri) = self.extract_data_image_uri_from_line(trimmed) {
                if let Some(attachment) = self.decode_data_image_uri_to_attachment(data_uri) {
                    attachments.push(attachment);
                    continue;
                } else {
                    warn!("Failed to decode inline data URI image; leaving text as-is");
                }
            }
            kept_lines.push(line.to_string());
        }

        (kept_lines.join("\n"), attachments)
    }

    fn extract_data_image_uri_from_line<'a>(&self, line: &'a str) -> Option<&'a str> {
        if line.starts_with("data:image/") {
            return Some(line);
        }
        if line.starts_with("![") && line.ends_with(')') {
            if let Some(paren_idx) = line.rfind('(') {
                let uri = &line[paren_idx + 1..line.len().saturating_sub(1)];
                if uri.starts_with("data:image/") {
                    return Some(uri);
                }
            }
        }
        None
    }

    fn decode_data_image_uri_to_attachment(&self, uri: &str) -> Option<OutboundAttachment> {
        let payload = uri.strip_prefix("data:image/")?;
        let semicolon_idx = payload.find(';')?;
        let subtype_raw = &payload[..semicolon_idx];
        let rest = &payload[semicolon_idx..];
        let marker = ";base64,";
        let marker_idx = rest.find(marker)?;
        let b64 = &rest[marker_idx + marker.len()..];

        let normalized_b64: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
        if normalized_b64.is_empty() {
            return None;
        }

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(normalized_b64.as_bytes())
            .ok()?;
        if bytes.is_empty() || bytes.len() > MAX_INLINE_IMAGE_BYTES {
            return None;
        }

        let subtype = subtype_raw.to_ascii_lowercase();
        let (kind, ext) = match subtype.as_str() {
            "png" => (AttachmentKind::Image, "png"),
            "jpg" | "jpeg" => (AttachmentKind::Image, "jpg"),
            "webp" => (AttachmentKind::Image, "webp"),
            "gif" => (AttachmentKind::Image, "gif"),
            "bmp" => (AttachmentKind::Image, "bmp"),
            "svg" | "svg+xml" => (AttachmentKind::Document, "svg"),
            _ => (AttachmentKind::Document, "bin"),
        };

        let dir = std::env::temp_dir().join("openpaw-telegram-inline-images");
        if std::fs::create_dir_all(&dir).is_err() {
            return None;
        }
        let path = dir.join(format!("img-{}.{}", Uuid::new_v4(), ext));
        if std::fs::write(&path, &bytes).is_err() {
            return None;
        }

        Some(OutboundAttachment { kind, path })
    }

    fn resolve_attachment_path(&self, raw_path: &str) -> Option<PathBuf> {
        let input = PathBuf::from(raw_path);
        let candidate = if input.is_absolute() {
            input
        } else {
            std::env::current_dir().ok()?.join(input)
        };
        std::fs::canonicalize(candidate).ok()
    }

    // ── outbound attachments (teloxide) ─────────────────────────────────────

    fn send_attachment(&self, chat_id: &str, attachment: &OutboundAttachment) -> Result<()> {
        match attachment.kind {
            AttachmentKind::Image => self.send_photo(chat_id, &attachment.path),
            AttachmentKind::Document => self.send_document(chat_id, &attachment.path),
            AttachmentKind::Audio => self.send_audio(chat_id, &attachment.path),
        }
    }

    fn send_document(&self, chat_id: &str, path: &Path) -> Result<()> {
        let url = self.api_url("sendDocument");
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("document.bin")
            .to_string();
        let bytes = std::fs::read(path)?;
        let form = multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part(
                "document",
                multipart::Part::bytes(bytes).file_name(file_name),
            );
        let response = self.client.post(&url).multipart(form).send()?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() || !body.contains("\"ok\":true") {
            return Err(anyhow!(
                "Telegram sendDocument error for {}: {} - {}",
                path.display(),
                status,
                body
            ));
        }
        Ok(())
    }

    fn send_photo(&self, chat_id: &str, path: &Path) -> Result<()> {
        let url = self.api_url("sendPhoto");
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image.bin")
            .to_string();
        let bytes = std::fs::read(path)?;
        let form = multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part("photo", multipart::Part::bytes(bytes).file_name(file_name));
        let response = self.client.post(&url).multipart(form).send()?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() || !body.contains("\"ok\":true") {
            return Err(anyhow!(
                "Telegram sendPhoto error for {}: {} - {}",
                path.display(),
                status,
                body
            ));
        }
        Ok(())
    }

    /// NEW: send audio via sendAudio (e.g. for [AUDIO:...] markers)
    fn send_audio(&self, chat_id: &str, path: &Path) -> Result<()> {
        let url = self.api_url("sendAudio");
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.mp3")
            .to_string();
        let bytes = std::fs::read(path)?;
        let form = multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part("audio", multipart::Part::bytes(bytes).file_name(file_name));
        let response = self.client.post(&url).multipart(form).send()?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() || !body.contains("\"ok\":true") {
            return Err(anyhow!(
                "Telegram sendAudio error for {}: {} - {}",
                path.display(),
                status,
                body
            ));
        }
        Ok(())
    }

    // ── low-level message send/edit ─────────────────────────────────────────

    fn send_single_message_internal(
        &self,
        chat_id: &str,
        text: &str,
        reply_to: Option<i64>,
        edit_message_id: Option<i64>,
    ) -> Result<i64> {
        let url = if edit_message_id.is_some() {
            self.api_url("editMessageText")
        } else {
            self.api_url("sendMessage")
        };

        let parsed_choices = parse_assistant_choices(text);
        let html_text = self.markdown_to_html(&parsed_choices.visible_text);

        let reply_markup = if let Some(choices) = parsed_choices.choices {
            let keyboard_buttons: Vec<Value> = choices
                .options
                .iter()
                .map(|opt| {
                    serde_json::json!({
                        "text": opt.label,
                        "callback_data": opt.id
                    })
                })
                .collect();
            let mut inline_keyboard: Vec<Vec<Value>> = Vec::new();
            for chunk in keyboard_buttons.chunks(3) {
                inline_keyboard.push(chunk.to_vec());
            }
            Some(serde_json::json!({ "inline_keyboard": inline_keyboard }))
        } else {
            None
        };

        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "text": html_text,
            "parse_mode": "HTML"
        });

        if let Some(msg_id) = edit_message_id {
            body["message_id"] = Value::Number(msg_id.into());
        }
        if let Some(reply_id) = reply_to {
            body["reply_to_message_id"] = Value::Number(reply_id.into());
        }
        if let Some(markup) = reply_markup {
            body["reply_markup"] = markup;
        }

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()?;

        let status = response.status();
        let resp_text = response.text()?;

        if !status.is_success() {
            if resp_text.contains("can't parse") || resp_text.contains("parse") {
                let plain_text = parsed_choices.visible_text;
                let mut plain_body = serde_json::json!({
                    "chat_id": chat_id,
                    "text": plain_text
                });
                if let Some(reply_id) = reply_to {
                    plain_body["reply_to_message_id"] = Value::Number(reply_id.into());
                }
                let plain_response = self
                    .client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .json(&plain_body)
                    .send()?;
                if !plain_response.status().is_success() {
                    return Err(anyhow!(
                        "Telegram API error (plain): {} - {}",
                        plain_response.status(),
                        plain_response.text()?
                    ));
                }
                let parsed: Value = serde_json::from_str(&plain_response.text()?)?;
                return Ok(parsed
                    .get("result")
                    .and_then(|r| r.get("message_id"))
                    .and_then(|m| m.as_i64())
                    .unwrap_or(0));
            }
            if resp_text.contains("message is not modified") {
                return Ok(edit_message_id.unwrap_or(0));
            }
            return Err(anyhow!("Telegram API error: {} - {}", status, resp_text));
        }

        if resp_text.contains("\"ok\":false") {
            if resp_text.contains("message is not modified") {
                return Ok(edit_message_id.unwrap_or(0));
            }
            return Err(anyhow!("Telegram API returned error: {}", resp_text));
        }

        let parsed: Value = serde_json::from_str(&resp_text)?;
        Ok(parsed
            .get("result")
            .and_then(|r| r.get("message_id"))
            .and_then(|m| m.as_i64())
            .unwrap_or(0))
    }

    fn send_single_message(&self, chat_id: &str, text: &str, reply_to: Option<i64>) -> Result<()> {
        self.send_single_message_internal(chat_id, text, reply_to, None)?;
        Ok(())
    }
}

// ── Channel trait impl ──────────────────────────────────────────────────────

impl Channel for TelegramChannel {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "telegram"
    }

    fn account_id(&self) -> &str {
        &self.config.account_id
    }

    fn send_message(&self, chat_id: &str, text: &str) -> Result<()> {
        // chat_id arriving from the bus may have a "telegram:" routing prefix
        // (stored as "telegram:<numeric_id>" in ParsedMessage). Strip it so the
        // Telegram API receives only the raw numeric chat id.
        let chat_id = chat_id.strip_prefix("telegram:").unwrap_or(chat_id);

        // Stop the typing heartbeat for this chat before sending the answer
        self.stop_typing_heartbeat(chat_id);

        let edit_id = {
            let mut streams = self.active_streams.lock().unwrap();
            streams.remove(chat_id)
        };

        let (cleaned_text, attachments) = self.parse_outbound_attachments(text);
        let text_to_send = cleaned_text.trim();
        let mut delivered_via_edit = false;

        if let Some(msg_id) = edit_id {
            if !text_to_send.is_empty() && text_to_send.len() <= MAX_MESSAGE_LEN {
                match self.send_single_message_internal(chat_id, text_to_send, None, Some(msg_id)) {
                    Ok(_) => delivered_via_edit = true,
                    Err(e) => warn!(
                        "Failed to finalize streamed Telegram message via edit: {}",
                        e
                    ),
                }
            } else if text_to_send.is_empty() && !attachments.is_empty() {
                match self.send_single_message_internal(
                    chat_id,
                    "Sent attachment.",
                    None,
                    Some(msg_id),
                ) {
                    Ok(_) => delivered_via_edit = true,
                    Err(e) => warn!(
                        "Failed to finalize attachment stream marker via edit: {}",
                        e
                    ),
                }
            }
        }

        if !delivered_via_edit && !text_to_send.is_empty() {
            self.send_message_with_splitting(chat_id, text_to_send, None)?;
        } else if !delivered_via_edit && attachments.is_empty() {
            self.send_message_with_splitting(chat_id, "...", None)?;
        }

        for attachment in &attachments {
            self.send_attachment(chat_id, attachment)?;
        }

        Ok(())
    }

    fn send_stream_chunk(&self, chat_id: &str, text: &str) -> Result<()> {
        let chat_id = chat_id.strip_prefix("telegram:").unwrap_or(chat_id);
        let text_to_send = if text.is_empty() { "..." } else { text };

        let safe_text = if text_to_send.len() > MAX_MESSAGE_LEN {
            let mut boundary = MAX_MESSAGE_LEN - 3;
            while !text_to_send.is_char_boundary(boundary) && boundary > 0 {
                boundary -= 1;
            }
            format!("{}...", &text_to_send[..boundary])
        } else {
            text_to_send.to_string()
        };

        let msg_id_opt = {
            let streams = self.active_streams.lock().unwrap();
            streams.get(chat_id).copied()
        };

        if let Some(msg_id) = msg_id_opt {
            let _ = self.send_single_message_internal(chat_id, &safe_text, None, Some(msg_id));
        } else {
            if let Ok(new_id) = self.send_single_message_internal(chat_id, &safe_text, None, None) {
                if new_id != 0 {
                    let mut streams = self.active_streams.lock().unwrap();
                    streams.insert(chat_id.to_string(), new_id);
                }
            }
        }
        Ok(())
    }

    /// Single-fire typing indicator; also starts the continuous heartbeat so
    /// the user sees feedback throughout long agent turns.
    fn send_typing(&self, chat_id: &str) {
        self.send_typing_once(chat_id);

        // Spin up a heartbeat if one isn't already running for this chat
        let already_running = {
            let map = self.typing_stops.lock().unwrap();
            map.contains_key(chat_id)
        };
        if !already_running {
            let stop = self.start_typing_heartbeat(chat_id);
            let mut map = self.typing_stops.lock().unwrap();
            map.insert(chat_id.to_string(), stop);
        }
    }

    /// Poll for updates using teloxide's Bot::get_updates via a block_on call.
    /// The Channel trait requires a sync return so we run the async call
    /// synchronously using the current tokio runtime handle.
    fn poll_updates(&self) -> Result<Vec<ParsedMessage>> {
        let offset = self.last_update_id.load(Ordering::Relaxed);

        // Use reqwest blocking to call getUpdates (long-polling, 30 s timeout)
        let url = self.api_url("getUpdates");
        let body = serde_json::json!({
            "offset": offset,
            "timeout": 30,
            "allowed_updates": ["message", "callback_query"]
        });

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(Duration::from_secs(45))
            .send()?;

        let resp_text = response.text()?;
        let parsed: Value = serde_json::from_str(&resp_text)?;

        if !parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Err(anyhow!("Telegram API error: {}", resp_text));
        }

        let result = match parsed.get("result").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => return Ok(Vec::new()),
        };

        let mut messages = Vec::new();
        for update in result {
            self.process_update(update, &mut messages);
        }

        Ok(messages)
    }

    fn health_check(&self) -> bool {
        let url = self.api_url("getMe");
        match self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({}))
            .timeout(Duration::from_secs(10))
            .send()
        {
            Ok(resp) => {
                if let Ok(text) = resp.text() {
                    text.contains("\"ok\":true")
                } else {
                    false
                }
            }
            Err(_) => false,
        }
    }
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_channel() -> TelegramChannel {
        TelegramChannel::new(TelegramConfig {
            account_id: "test".to_string(),
            bot_token: "test_token".to_string(),
            allow_from: vec!["*".to_string()],
            group_allow_from: vec![],
            group_policy: "allowlist".to_string(),
            reply_in_private: true,
            proxy: None,
        })
    }

    #[test]
    fn test_markdown_to_html() {
        let ch = make_channel();
        let html = ch.markdown_to_html("Hello **bold** and *italic* and `code`!");
        assert_eq!(
            html,
            "Hello <b>bold</b> and <i>italic</i> and <code>code</code>!"
        );

        let html_block = ch.markdown_to_html("```\ncode block\n```");
        assert_eq!(html_block, "<pre><code>code block\n</code></pre>");
    }

    #[test]
    fn test_smart_split() {
        let ch = make_channel();
        let text = "a".repeat(5000);
        let chunks = ch.smart_split(&text, 100);
        assert!(!chunks.is_empty());
        assert!(chunks[0].len() <= 100);

        let text2 = "line1\nline2\nline3";
        let chunks2 = ch.smart_split(text2, 20);
        assert_eq!(chunks2.len(), 1);
    }

    #[test]
    fn test_user_allowed() {
        let config = TelegramConfig {
            account_id: "test".to_string(),
            bot_token: "test_token".to_string(),
            allow_from: vec!["@testuser".to_string(), "123456".to_string()],
            group_allow_from: vec![],
            group_policy: "allowlist".to_string(),
            reply_in_private: true,
            proxy: None,
        };
        let ch = TelegramChannel::new(config);
        assert!(ch.is_user_allowed("testuser", ""));
        assert!(ch.is_user_allowed("TestUser", "")); // case-insensitive
        assert!(ch.is_user_allowed("", "123456")); // by user ID
        assert!(!ch.is_user_allowed("otheruser", ""));
    }

    #[test]
    fn test_extract_content_text() {
        let ch = make_channel();
        let msg = serde_json::json!({ "text": "Hello world" });
        assert_eq!(ch.extract_message_content(&msg), "Hello world");
    }

    #[test]
    fn test_extract_content_caption_only() {
        let ch = make_channel();
        let msg = serde_json::json!({ "caption": "Photo caption" });
        assert_eq!(ch.extract_message_content(&msg), "Photo caption");
    }

    #[test]
    fn test_parse_outbound_attachments_document_marker() {
        let ch = make_channel();
        let tmp = std::env::temp_dir().join("openpaw-telegram-marker-test.txt");
        std::fs::write(&tmp, "hello").unwrap();
        let input = format!("Done.\n[FILE:{}]", tmp.display());
        let (cleaned, attachments) = ch.parse_outbound_attachments(&input);
        assert_eq!(cleaned, "Done.");
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].kind, AttachmentKind::Document);
        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn test_parse_outbound_attachments_image_marker() {
        let ch = make_channel();
        let tmp = std::env::temp_dir().join("openpaw-telegram-marker-test.png");
        std::fs::write(&tmp, "png").unwrap();
        let input = format!("[IMAGE:{}]", tmp.display());
        let (cleaned, attachments) = ch.parse_outbound_attachments(&input);
        assert!(cleaned.is_empty());
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].kind, AttachmentKind::Image);
        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn test_parse_outbound_attachments_audio_marker() {
        let ch = make_channel();
        let tmp = std::env::temp_dir().join("openpaw-telegram-marker-test.mp3");
        std::fs::write(&tmp, "audio").unwrap();
        let input = format!("[AUDIO:{}]", tmp.display());
        let (cleaned, attachments) = ch.parse_outbound_attachments(&input);
        assert!(cleaned.is_empty());
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].kind, AttachmentKind::Audio);
        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn test_parse_outbound_attachments_inline_data_uri_line() {
        let ch = make_channel();
        let input = "Here is an image:\n\ndata:image/png;base64,aGVsbG8=\n\nDone";
        let (cleaned, attachments) = ch.parse_outbound_attachments(input);
        assert_eq!(cleaned, "Here is an image:\n\nDone");
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].kind, AttachmentKind::Image);
        let _ = std::fs::remove_file(&attachments[0].path);
    }

    #[test]
    fn test_parse_outbound_attachments_inline_data_uri_markdown() {
        let ch = make_channel();
        let input = "![preview](data:image/png;base64,aGVsbG8=)";
        let (cleaned, attachments) = ch.parse_outbound_attachments(input);
        assert!(cleaned.is_empty());
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].kind, AttachmentKind::Image);
        let _ = std::fs::remove_file(&attachments[0].path);
    }

    #[test]
    fn test_typing_heartbeat_starts_and_stops() {
        let ch = make_channel();
        // start heartbeat
        let stop = ch.start_typing_heartbeat("999");
        assert!(!stop.load(Ordering::Relaxed));
        // stop it
        stop.store(true, Ordering::Relaxed);
    }
}
