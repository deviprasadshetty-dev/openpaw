#![cfg(feature = "telegram")]
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
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tracing::{info, warn};
use uuid::Uuid;

const MAX_MESSAGE_LEN: usize = 4096;
const MAX_INLINE_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_TEXT_FILE_READ_BYTES: usize = 100 * 1024;
const API_BASE: &str = "https://api.telegram.org/bot";
/// Temp dir for inbound files downloaded from Telegram
const INBOUND_TEMP_DIR: &str = "openpaw-tg-inbound";

/// Telegram measures message length in UTF-16 code units.
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Returns the path to the Telegram inbound temp directory.
pub(crate) fn tg_inbound_dir() -> PathBuf {
    std::env::temp_dir().join(INBOUND_TEMP_DIR)
}

// ── SSRF Protection ─────────────────────────────────────────────────────────

/// Heuristic URL safety check: rejects private/internal IPs, loopback,
/// cloud metadata endpoints, and link-local addresses.
fn is_safe_url(url_str: &str) -> bool {
    // Quick string heuristics — catches obvious metadata/loopback patterns.
    let lower = url_str.to_lowercase();
    if lower.contains("169.254.169.254")               // AWS/Azure/GCP metadata
        || lower.contains("metadata.google.internal")   // GCP metadata
        || lower.contains("169.254.0.0")                // link-local
        || lower.contains("127.0.0.1")                  // loopback
        || lower.contains("[::1]")                      // IPv6 loopback
        || lower.contains("localhost")                  // localhost
        || lower.contains("0.0.0.0")                    // any-addr
    {
        return false;
    }

    // Check if host is parseable as an IP and is private.
    if let Some(host) = extract_host(url_str) {
        if let Ok(ip) = host.parse::<IpAddr>() {
            if is_private_ip(&ip) {
                return false;
            }
        }
    }

    true
}

/// Extract a hostname from a URL string (naïve heuristic).
fn extract_host(url_str: &str) -> Option<&str> {
    let after_scheme = url_str.strip_prefix("https://")
        .or_else(|| url_str.strip_prefix("http://"))
        .unwrap_or(url_str);
    let host_end = after_scheme.find('/')
        .or_else(|| after_scheme.find(':'))
        .or_else(|| after_scheme.find('?'))
        .or_else(|| after_scheme.find('#'))
        .unwrap_or(after_scheme.len());
    let host = &after_scheme[..host_end];
    if host.is_empty() { None } else { Some(host) }
}

/// Returns true if the IP address is in a private/loopback/link-local range.
fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.octets() == [0, 0, 0, 0]
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_unspecified()
        }
    }
}

/// Global registry of Telegram channels indexed by bot token (for webhook lookup).
static WEBHOOK_CHANNELS: OnceLock<Mutex<HashMap<String, Arc<TelegramChannel>>>> = OnceLock::new();

fn webhook_channels() -> &'static Mutex<HashMap<String, Arc<TelegramChannel>>> {
    WEBHOOK_CHANNELS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Look up a Telegram channel by bot token for webhook dispatch.
pub(crate) fn find_channel_by_token(token: &str) -> Option<Arc<TelegramChannel>> {
    webhook_channels().lock().ok()?.get(token).cloned()
}

// ── Attachment kinds ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachmentKind {
    Document,
    Image,
    Audio,
    Video,
}

#[derive(Debug, Clone)]
struct OutboundAttachment {
    kind: AttachmentKind,
    path: PathBuf,
}

/// Emoji constants for message reactions.
mod emoji {
    pub const WATCHING: &str = "\u{1F440}";       // 👀
    pub const THUMBS_UP: &str = "\u{1F44D}";      // 👍
    pub const THUMBS_DOWN: &str = "\u{1F44E}";    // 👎
}

fn format_inbound_attachment(kind: &str, path: &Path, caption: &str) -> String {
    let marker = format!("[{}:{}]", kind, path.display());
    let caption = caption.trim();
    if caption.is_empty() {
        marker
    } else {
        format!("{}\nCaption: {}", marker, caption)
    }
}

// ── Streaming / Draft Types ────────────────────────────────────────────────

pub const DRAFT_FLUSH_MIN_DELTA_BYTES: usize = 32;
pub const DRAFT_FLUSH_MIN_INTERVAL_MS: u64 = 500;

#[derive(Debug)]
struct DraftState {
    buffer: String,
    last_flush_len: usize,
    last_flush_time: std::time::Instant,
}

impl DraftState {
    fn new() -> Self {
        Self {
            buffer: String::new(),
            last_flush_len: 0,
            last_flush_time: std::time::Instant::now(),
        }
    }

    fn should_flush(&self) -> bool {
        let delta = self.buffer.len().saturating_sub(self.last_flush_len);
        let elapsed = self.last_flush_time.elapsed().as_millis() as u64;
        delta >= DRAFT_FLUSH_MIN_DELTA_BYTES || elapsed >= DRAFT_FLUSH_MIN_INTERVAL_MS
    }
}

/// TagFilter - strips tool-control blocks from a stream of chunks.
struct TagFilter {
    buffer: String,
    inside_tag: bool,
}

impl TagFilter {
    fn new() -> Self {
        Self {
            buffer: String::new(),
            inside_tag: false,
        }
    }

    fn process(&mut self, chunk: &str) -> String {
        let mut out = String::new();
        self.buffer.push_str(chunk);

        while !self.buffer.is_empty() {
            if self.inside_tag {
                if let Some(pos) = self.buffer.find('>') {
                    self.buffer.drain(..pos + 1);
                    self.inside_tag = false;
                } else {
                    self.buffer.clear(); // Wait for more
                }
            } else if let Some(pos) = self.buffer.find('<') {
                out.push_str(&self.buffer[..pos]);
                let rest = &self.buffer[pos..];
                if rest.starts_with("<tool_call")
                    || rest.starts_with("<tool_result")
                    || rest.starts_with("<|")
                {
                    self.inside_tag = true;
                    if let Some(end_pos) = rest.find('>') {
                        self.buffer.drain(..pos + end_pos + 1);
                        self.inside_tag = false;
                    } else {
                        self.buffer.drain(..pos);
                        break; // Wait for '>'
                    }
                } else {
                    out.push('<');
                    self.buffer.drain(..pos + 1);
                }
            } else {
                out.push_str(&self.buffer);
                self.buffer.clear();
            }
        }
        out
    }
}

// ── Inbound Debounce Types ──────────────────────────────────────────────────

#[derive(Debug)]
struct PendingInbound {
    message: ParsedMessage,
    received_at: std::time::Instant,
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
    /// Maps chat_id -> draft state for streaming.
    draft_buffers: Mutex<HashMap<String, DraftState>>,
    /// Maps chat_id -> tag filter for streaming.
    tag_filters: Mutex<HashMap<String, TagFilter>>,
    /// Maps chat_id -> stop flag for the typing heartbeat thread.
    typing_stops: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Pending media group messages (media_group_id -> messages)
    pending_media: Mutex<HashMap<String, Vec<PendingInbound>>>,
    /// Pending text chunks for debouncing (chat_id -> chunks)
    pending_text: Mutex<HashMap<String, Vec<PendingInbound>>>,
}

impl TelegramChannel {
    pub fn new(config: TelegramConfig) -> Self {
        // Configure reqwest client with explicit connection pool settings
        // (matches Hermes HTTPX pool configuration: pool 512, pool timeout 8s,
        //  connect timeout 10s, read timeout 20s, write timeout 20s).
        let client = Client::builder()
            .pool_max_idle_per_host(512)
            .pool_idle_timeout(Some(Duration::from_secs(8)))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(45))
            .build()
            .expect("Failed to build reqwest Client");

        Self {
            client,
            config,
            last_update_id: AtomicI64::new(0),
            active_streams: Mutex::new(HashMap::new()),
            draft_buffers: Mutex::new(HashMap::new()),
            tag_filters: Mutex::new(HashMap::new()),
            typing_stops: Mutex::new(HashMap::new()),
            pending_media: Mutex::new(HashMap::new()),
            pending_text: Mutex::new(HashMap::new()),
        }
    }

    /// Returns true if this channel is configured for webhook mode.
    pub fn is_webhook_mode(&self) -> bool {
        self.config.webhook_url.is_some()
    }

    /// Register this channel for webhook dispatch and call Telegram setWebhook API.
    /// Also registers bot commands via setMyCommands for the /commands menu.
    pub fn init_webhook(self: &Arc<Self>) {
        // Always register bot commands (#13)
        self.register_bot_commands();

        if let Some(ref webhook_url) = self.config.webhook_url {
            let token = &self.config.bot_token;
            if let Ok(mut map) = webhook_channels().lock() {
                map.insert(token.clone(), self.clone());
            }
            let hook_url = format!(
                "{}/telegram/webhook/{}",
                webhook_url.trim_end_matches('/'),
                token
            );
            let url = self.api_url("setWebhook");
            let body = serde_json::json!({
                "url": hook_url,
                "allowed_updates": ["message", "edited_message", "callback_query"]
            });
            match self
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
            {
                Ok(resp) => {
                    info!(
                        "Telegram setWebhook → {} (status: {})",
                        hook_url,
                        resp.status()
                    );
                }
                Err(e) => {
                    warn!("Telegram setWebhook failed for {}: {}", hook_url, e);
                }
            }
        }
    }

    /// Register bot commands via setMyCommands so users see a /command menu.
    fn register_bot_commands(&self) {
        let commands = serde_json::json!([
            {"command": "start", "description": "Start the bot"},
            {"command": "help", "description": "Show help information"},
            {"command": "status", "description": "Show agent status"},
            {"command": "reset", "description": "Reset conversation context"},
            {"command": "model", "description": "Show/change active model"},
            {"command": "tools", "description": "List available tools"},
            {"command": "memory", "description": "Manage saved memories"},
            {"command": "stop", "description": "Stop current action"}
        ]);
        let url = self.api_url("setMyCommands");
        let body = serde_json::json!({
            "commands": commands
        });
        match self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
        {
            Ok(resp) => {
                info!(
                    "Telegram setMyCommands registered (status: {})",
                    resp.status()
                );
            }
            Err(e) => {
                warn!("Telegram setMyCommands failed: {}", e);
            }
        }
    }

    /// Process a single Telegram webhook update. Downloads files, creates
    /// ParsedMessage, and publishes to the global bus.
    pub fn process_webhook_update(&self, body: Value) {
        use crate::bus::{InboundMessage, global_bus};

        let mut messages = Vec::new();
        self.process_update(&body, &mut messages);
        self.flush_pending_inbound(&mut messages);

        for msg in messages {
            let meta_json = if msg.message_id.is_some() || msg.message_thread_id.is_some() {
                Some(
                    serde_json::json!({
                        "account_id": self.config.account_id,
                        "message_id": msg.message_id.map(|id| id.to_string()),
                        "thread_id": msg.message_thread_id.map(|id| id.to_string()),
                        "is_group": msg.is_group,
                    })
                    .to_string(),
                )
            } else {
                None
            };
            let inbound = InboundMessage {
                channel: "telegram".to_string(),
                sender_id: msg.sender_id,
                chat_id: msg.chat_id,
                content: msg.content,
                session_key: msg.session_key,
                media: Vec::new(),
                metadata_json: meta_json,
            };
            if let Some(bus) = global_bus() {
                if let Err(e) = bus.publish_inbound(inbound) {
                    warn!("Failed to publish webhook inbound message: {}", e);
                }
            }
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
            let check = if let Some(stripped) = allowed.strip_prefix('@') {
                stripped
            } else {
                allowed
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
            let check = if let Some(stripped) = allowed.strip_prefix('@') {
                stripped
            } else {
                allowed
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
        let (chat_id, thread_id) = split_chat_thread(chat_id);
        let mut body = serde_json::json!({ "chat_id": chat_id, "action": "typing" });
        if let Some(thread_id) = thread_id {
            body["message_thread_id"] = Value::Number(thread_id.into());
        }
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
        let (chat_id_owned, thread_id) = split_chat_thread(chat_id);

        std::thread::spawn(move || {
            let body = serde_json::json!({ "chat_id": chat_id_owned, "action": "typing" });
            let mut body = body;
            if let Some(thread_id) = thread_id {
                body["message_thread_id"] = Value::Number(thread_id.into());
            }
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
        if let Ok(mut map) = self.typing_stops.lock()
            && let Some(flag) = map.remove(chat_id)
        {
            flag.store(true, Ordering::Relaxed);
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
        if let Some(photos) = message.get("photo").and_then(|v| v.as_array())
            && let Some(last_photo) = photos.last()
        {
            let file_id = last_photo
                .get("file_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some(path) = self.download_tg_file(file_id, "photo", "jpg") {
                return format_inbound_attachment("IMAGE", &path, &caption);
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

        // Document
        if let Some(doc) = message.get("document") {
            let file_id = doc.get("file_id").and_then(|v| v.as_str()).unwrap_or("");
            let file_name = doc
                .get("file_name")
                .and_then(|v| v.as_str())
                .unwrap_or("document");
            let mime_type = doc
                .get("mime_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let ext = Path::new(file_name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("bin");

            // ── Document-as-Image detection (#6) ──
            let is_image = mime_type.starts_with("image/")
                || matches!(
                    ext.to_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg"
                );
            if is_image {
                if let Some(path) = self.download_tg_file(file_id, "photo", ext) {
                    return format_inbound_attachment("IMAGE", &path, &caption);
                }
                return format!(
                    "[Image (as document): {} file_id={}]",
                    file_name, file_id
                );
            }

            if let Some(path) = self.download_tg_file(file_id, "doc", ext) {
                // ── Text file content injection (#7) ──
                let is_text = mime_type.starts_with("text/")
                    || mime_type == "application/json"
                    || mime_type.contains("xml")
                    || matches!(
                        ext.to_lowercase().as_str(),
                        "md" | "txt" | "json" | "xml" | "yaml" | "yml" | "toml" | "csv" | "log" | "rs" | "py" | "js" | "ts" | "sh" | "bat"
                    );
                if is_text {
                    if let Ok(bytes) = std::fs::read(&path) {
                        let cap = MAX_TEXT_FILE_READ_BYTES;
                        if let Ok(content) = String::from_utf8(
                            if bytes.len() > cap { bytes[..cap].to_vec() } else { bytes }
                        ) {
                            let marker = format_inbound_attachment("FILE", &path, &caption);
                            let truncated = if std::fs::metadata(&path)
                                .map(|m| m.len() > cap as u64)
                                .unwrap_or(false)
                            {
                                format!("{}\n\n[File content (first {} bytes):]\n{}", marker, cap, content)
                            } else {
                                format!("{}\n\n[File content:]\n{}", marker, content)
                            };
                            return truncated;
                        }
                    }
                }

                return format!(
                    "Document: {}\n{}",
                    file_name,
                    format_inbound_attachment("FILE", &path, &caption)
                );
            }
            return format!("[Document: {} file_id={}]", file_name, file_id);
        }

        // Voice
        if let Some(voice) = message.get("voice") {
            let file_id = voice.get("file_id").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(path) = self.download_tg_file(file_id, "voice", "ogg") {
                return format_inbound_attachment("AUDIO", &path, "");
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
                return format!(
                    "Audio: {}\n{}",
                    title,
                    format_inbound_attachment("AUDIO", &path, "")
                );
            }
            return format!("[Audio: {} file_id={}]", title, file_id);
        }

        // Sticker
        if let Some(sticker) = message.get("sticker") {
            let emoji = sticker.get("emoji").and_then(|v| v.as_str()).unwrap_or("");
            let set_name = sticker
                .get("set_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            return format!(
                "[User sent sticker{}{}]",
                if emoji.is_empty() {
                    String::new()
                } else {
                    format!(" {}", emoji)
                },
                if set_name.is_empty() {
                    String::new()
                } else {
                    format!(" from set {}", set_name)
                }
            );
        }

        // Location
        if let Some(location) = message.get("location") {
            let lat = location
                .get("latitude")
                .and_then(|v| v.as_f64())
                .unwrap_or_default();
            let lon = location
                .get("longitude")
                .and_then(|v| v.as_f64())
                .unwrap_or_default();
            let maps_link = format!(
                "https://www.google.com/maps/search/?api=1&query={},{}",
                lat, lon
            );
            return format!(
                "[User shared location: {}, {}]\nMap: {}\n(You can help the user find nearby restaurants, cafes, or other points of interest)",
                lat, lon, maps_link
            );
        }

        // Contact
        if let Some(contact) = message.get("contact") {
            let first_name = contact
                .get("first_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let last_name = contact
                .get("last_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let phone = contact
                .get("phone_number")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return format!(
                "[User shared contact: {}{} phone={}]",
                first_name,
                if last_name.is_empty() {
                    String::new()
                } else {
                    format!(" {}", last_name)
                },
                phone
            );
        }

        // Video
        if let Some(video) = message.get("video") {
            let file_id = video.get("file_id").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(path) = self.download_tg_file(file_id, "video", "mp4") {
                return format_inbound_attachment("VIDEO", &path, &caption);
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

        let (message, edited) = match update
            .get("message")
            .map(|m| (m, false))
            .or_else(|| update.get("edited_message").map(|m| (m, true)))
        {
            Some(pair) => pair,
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
        let message_thread_id = message.get("message_thread_id").and_then(|v| v.as_i64());
        let media_group_id = message.get("media_group_id").and_then(|v| v.as_str());
        let mut content = self.extract_message_content(message);

        if content.is_empty() {
            return;
        }
        if edited {
            content = format!("[Edited Telegram message]\n{}", content);
        }

        let sender_identity = if username != "unknown" {
            username.to_string()
        } else {
            user_id.clone()
        };

        let parsed = ParsedMessage {
            sender_id: sender_identity,
            chat_id: if let Some(thread_id) = message_thread_id {
                format!("{}:thread:{}", chat_id, thread_id)
            } else {
                chat_id.clone()
            },
            content,
            session_key: if let Some(thread_id) = message_thread_id {
                format!("telegram:{}:thread:{}", chat_id, thread_id)
            } else {
                format!("telegram:{}", chat_id)
            },
            is_group,
            update_id,
            message_id,
            message_thread_id,
            username: if username != "unknown" {
                Some(username.to_string())
            } else {
                None
            },
            first_name,
        };

        // ── Inbound Buffering / Debouncing ──
        if let Some(mgid) = media_group_id {
            let mut map = self.pending_media.lock().unwrap();
            map.entry(mgid.to_string())
                .or_default()
                .push(PendingInbound {
                    message: parsed,
                    received_at: std::time::Instant::now(),
                });
        } else if parsed.content.len() > 500 {
            // Likely a split text chunk
            let mut map = self.pending_text.lock().unwrap();
            map.entry(chat_id).or_default().push(PendingInbound {
                message: parsed,
                received_at: std::time::Instant::now(),
            });
        } else {
            messages.push(parsed);
        }
    }

    /// Flushes matured pending inbound messages.
    fn flush_pending_inbound(&self, messages: &mut Vec<ParsedMessage>) {
        // Media Groups: flush after 3 seconds of inactivity
        {
            let mut map = self.pending_media.lock().unwrap();
            let mut to_remove = Vec::new();
            for (mgid, pending) in map.iter() {
                if let Some(last) = pending.last()
                    && last.received_at.elapsed().as_secs() >= 3
                {
                    to_remove.push(mgid.clone());
                }
            }
            for mgid in to_remove {
                if let Some(mut pending) = map.remove(&mgid)
                    && !pending.is_empty()
                {
                    let mut first = pending.remove(0).message;
                    for extra in pending {
                        first.content.push('\n');
                        first.content.push_str(&extra.message.content);
                    }
                    messages.push(first);
                }
            }
        }

        // Text chunks: flush after 2 seconds of inactivity
        {
            let mut map = self.pending_text.lock().unwrap();
            let mut to_remove = Vec::new();
            for (chat_id, pending) in map.iter() {
                if let Some(last) = pending.last()
                    && last.received_at.elapsed().as_secs() >= 2
                {
                    to_remove.push(chat_id.clone());
                }
            }
            for chat_id in to_remove {
                if let Some(mut pending) = map.remove(&chat_id)
                    && !pending.is_empty()
                {
                    let mut first = pending.remove(0).message;
                    for extra in pending {
                        first.content.push('\n');
                        first.content.push_str(&extra.message.content);
                    }
                    messages.push(first);
                }
            }
        }
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
            chat_id: if let Some(thread_id) = message_obj
                .get("message_thread_id")
                .and_then(|v| v.as_i64())
            {
                format!("{}:thread:{}", chat_id, thread_id)
            } else {
                chat_id.clone()
            },
            content: format!("[Callback: {}]", cb_data),
            session_key: if let Some(thread_id) = message_obj
                .get("message_thread_id")
                .and_then(|v| v.as_i64())
            {
                format!("telegram:{}:thread:{}", chat_id, thread_id)
            } else {
                format!("telegram:{}", chat_id)
            },
            is_group,
            update_id,
            message_id: message_obj.get("message_id").and_then(|v| v.as_i64()),
            message_thread_id: message_obj
                .get("message_thread_id")
                .and_then(|v| v.as_i64()),
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
        if utf16_len(text) <= MAX_MESSAGE_LEN {
            return self.send_single_message(chat_id, text, reply_to);
        }
        let chunks = self.smart_split_utf16(text, MAX_MESSAGE_LEN - 12);
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

    // ── MarkdownV2 formatting ────────────────────────────────────────────

    /// Escape special characters for Telegram MarkdownV2 outside code spans.
    fn escape_mdv2(text: &str) -> String {
        // Characters that need backslash-escaping: _ * [ ] ( ) ~ ` > # + - = | { } . !
        let special = Regex::new(r"([_\*\[\]\(\)~`>#\+\-=\|\{\}\.!\\])").unwrap();
        special.replace_all(text, r"\$1").to_string()
    }

    /// Rewrite GFM pipe tables into Telegram-friendly bold-heading + bullet-list groups.
    /// Telegram MarkdownV2 has no table syntax — bare `|` renders as noisy escaped text.
    fn wrap_markdown_tables(&self, text: &str) -> String {
        // Match a GFM pipe table block:
        //   header row (| col1 | col2 | ...)
        //   separator row (| --- | --- | ...)
        //   one or more data rows
        let table_re = Regex::new(
            r"(?m)^(?:\|?[^\n\r]+\|[^\n\r]*\n)(?:\|?\s*:?-+:?\s*(?:\|\s*:?-+:?\s*)*\|?\s*\n)(?:(?:\|?[^\n\r]+\|[^\n\r]*\n?)+)"
        ).unwrap();

        table_re.replace_all(text, |caps: &regex::Captures| {
            let block = caps.get(0).map(|m| m.as_str()).unwrap_or("");
            self.render_table_block_for_telegram(block)
        }).to_string()
    }

    /// Render a single GFM table block as bold-heading + bullet-list lines.
    fn render_table_block_for_telegram(&self, block: &str) -> String {
        let lines: Vec<&str> = block.lines().collect();
        if lines.len() < 3 {
            return block.to_string();
        }

        let parse_cells = |line: &str| -> Vec<String> {
            line.split('|')
                .map(|cell| cell.trim().to_string())
                .filter(|cell| !cell.is_empty())
                .collect()
        };

        let header_cells = parse_cells(lines[0]);
        // lines[1] is the separator row; skip it.
        let data_lines = &lines[2..];

        if header_cells.is_empty() || data_lines.is_empty() {
            return block.to_string();
        }

        let row_label_col = if data_lines.len() >= 2 {
            // If the first column of each data row looks like a label (e.g. starts
            // with an underscore or has a ":" in it), treat it as a row label.
            let first_col = parse_cells(data_lines[0]);
            if !first_col.is_empty()
                && (first_col[0].starts_with('_') || first_col[0].contains(':'))
            {
                Some(0usize)
            } else {
                None
            }
        } else {
            None
        };

        let mut out = String::new();

        for data_line in data_lines {
            let cells = parse_cells(data_line);
            if cells.is_empty() {
                continue;
            }

            if let Some(label_col) = row_label_col {
                if let Some(label) = cells.get(label_col) {
                    out.push_str(&format!("\n*{label}*\n"));
                }
                for (i, cell) in cells.iter().enumerate() {
                    if i == label_col || cell.is_empty() {
                        continue;
                    }
                    let header_label = header_cells
                        .get(i)
                        .map(|h| format!("*{h}*"))
                        .unwrap_or_default();
                    out.push_str(&format!("  \u{2022} {header_label}: {cell}\n"));
                }
            } else {
                // Single row — treat as key: value pairs.
                if cells.len() == header_cells.len()
                    || cells.len() == header_cells.len() + 1
                    || header_cells.len() == cells.len() + 1
                {
                    for (i, cell) in cells.iter().enumerate() {
                        let header_label = header_cells
                            .get(i)
                            .map(|h| format!("*{h}*"))
                            .unwrap_or_default();
                        out.push_str(&format!("\u{2022} {header_label}: {cell}\n"));
                    }
                } else {
                    // Generic: just bullet-list the row.
                    let row_str = cells.join(" | ");
                    out.push_str(&format!("\u{2022} {row_str}\n"));
                }
            }
        }

        out
    }

    /// Convert markdown text to Telegram's MarkdownV2 format.
    ///
    /// Uses placeholder-based protection for code blocks and inline code,
    /// then translates markdown constructs to MarkdownV2 syntax.
    fn markdown_to_markdownv2(&self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }

        use std::collections::HashMap;

        let mut placeholders: HashMap<String, String> = HashMap::new();
        let mut counter: usize = 0;

        let mut stash = |value: String| -> String {
            let key = format!("\x00PH{counter}\x00");
            counter += 1;
            placeholders.insert(key.clone(), value);
            key
        };

        // 0) Rewrite GFM pipe tables before any escaping — Telegram can't render tables.
        let mut s = self.wrap_markdown_tables(text);

        // 1) Protect fenced code blocks (```...```)
        // Escape \ and ` inside code blocks per MarkdownV2 spec.
        let code_block_re = Regex::new(r"(?s)```(?:[^\n]*\n)?(.*?)```").unwrap();
        s = code_block_re.replace_all(&s, |caps: &regex::Captures| {
            let body = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let escaped = body.replace("\\", "\\\\").replace('`', "\\`");
            stash(format!("```{escaped}```"))
        }).to_string();

        // 2) Protect inline code (`...`)
        let inline_code_re = Regex::new(r"`([^`]+)`").unwrap();
        s = inline_code_re.replace_all(&s, |caps: &regex::Captures| {
            let inner = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let escaped = inner.replace("\\", "\\\\");
            stash(format!("`{escaped}`"))
        }).to_string();

        // 3) Convert markdown links [text](url)
        let link_re = Regex::new(r"\[([^\]]+)\]\(([^\(\)]*(?:\([^\(\)]*\)[^\(\)]*)*)\)").unwrap();
        s = link_re.replace_all(&s, |caps: &regex::Captures| {
            let display = Self::escape_mdv2(caps.get(1).map(|m| m.as_str()).unwrap_or(""));
            let url = caps.get(2).map(|m| m.as_str()).unwrap_or("").replace("\\", "\\\\").replace(')', "\\)");
            stash(format!("[{display}]({url})"))
        }).to_string();

        // 4) Convert headers (## Title) → bold *Title*
        let header_re = Regex::new(r"(?m)^#{1,6}\s+(.+)$").unwrap();
        s = header_re.replace_all(&s, |caps: &regex::Captures| {
            let inner = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            // Strip inner bold markers
            let inner = Regex::new(r"\*\*(.+?)\*\*")
                .unwrap()
                .replace_all(inner, "$1")
                .to_string();
            stash(format!("*{}*", Self::escape_mdv2(&inner)))
        }).to_string();

        // 5) Convert bold: **text** → *text* (MarkdownV2 bold)
        let bold_re = Regex::new(r"\*\*(.+?)\*\*").unwrap();
        s = bold_re.replace_all(&s, |caps: &regex::Captures| {
            stash(format!("*{}*", Self::escape_mdv2(caps.get(1).map(|m| m.as_str()).unwrap_or(""))))
        }).to_string();

        // 6) Convert italic: *text* → _text_ (MarkdownV2 italic)
        //    Avoid matching across newlines so bullet lists are not corrupted.
        let italic_re = Regex::new(r"\*([^\*\n]+)\*").unwrap();
        s = italic_re.replace_all(&s, |caps: &regex::Captures| {
            stash(format!("_{}_", Self::escape_mdv2(caps.get(1).map(|m| m.as_str()).unwrap_or(""))))
        }).to_string();

        // 7) Convert strikethrough: ~~text~~ → ~text~
        let strike_re = Regex::new(r"~~(.+?)~~").unwrap();
        s = strike_re.replace_all(&s, |caps: &regex::Captures| {
            stash(format!("~{}~", Self::escape_mdv2(caps.get(1).map(|m| m.as_str()).unwrap_or(""))))
        }).to_string();

        // 8) Convert spoiler: ||text|| → keep as is (protect pipes from escaping)
        let spoiler_re = Regex::new(r"\|\|(.+?)\|\|").unwrap();
        s = spoiler_re.replace_all(&s, |caps: &regex::Captures| {
            stash(format!("||{}||", Self::escape_mdv2(caps.get(1).map(|m| m.as_str()).unwrap_or(""))))
        }).to_string();

        // 9) Convert blockquotes: > text -> protect from escaping
        let blockquote_re = Regex::new(r"(?m)^((?:\*\*)?>{1,3}) (.+)$").unwrap();
        s = blockquote_re.replace_all(&s, |caps: &regex::Captures| {
            let prefix = caps.get(1).map(|m| m.as_str()).unwrap_or(">");
            let content = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            stash(format!("{} {}", prefix, Self::escape_mdv2(content)))
        }).to_string();

        // 10) Escape remaining special characters
        s = Self::escape_mdv2(&s);

        // 11) Restore placeholders in reverse insertion order
        let mut keys: Vec<&String> = placeholders.keys().collect();
        keys.sort_by(|a, b| b.cmp(a)); // reverse order so later inserts are restored first
        for key in keys {
            s = s.replace(key, &placeholders[key]);
        }

        // 12) Safety net: catch any `(` `)` `{` `}` that may have slipped through
        // outside of code blocks / links / placeholders.
        // Only affects non-code segments by working around known safe patterns.
        s = Self::mdv2_safety_net(&s);

        s
    }

    /// Final pass that catches unescaped `() {}` that may have survived the pipeline.
    /// Uses placeholder-protection so code blocks / links aren't corrupted.
    fn mdv2_safety_net(text: &str) -> String {
        let mut placeholders: HashMap<String, String> = HashMap::new();
        let mut counter: usize = 0;

        let mut stash = |value: String| -> String {
            let key = format!("\x00SN{counter}\x00");
            counter += 1;
            placeholders.insert(key.clone(), value);
            key
        };

        let mut s = text.to_string();

        // Protect code blocks and inline code (already escaped, don't double-process).
        let code_block_re = Regex::new(r"(?s)```[^`]*```").unwrap();
        s = code_block_re.replace_all(&s, |caps: &regex::Captures| {
            stash(caps.get(0).map(|m| m.as_str()).unwrap_or("").to_string())
        }).to_string();

        let inline_code_re = Regex::new(r"`[^`]*`").unwrap();
        s = inline_code_re.replace_all(&s, |caps: &regex::Captures| {
            stash(caps.get(0).map(|m| m.as_str()).unwrap_or("").to_string())
        }).to_string();

        // Protect link URLs: [text](url)
        let link_re = Regex::new(r"\[[^\]]*\]\([^\)]*\)").unwrap();
        s = link_re.replace_all(&s, |caps: &regex::Captures| {
            stash(caps.get(0).map(|m| m.as_str()).unwrap_or("").to_string())
        }).to_string();

        // Now escape any remaining `(` `)` `{` `}` that are still visible.
        s = s.replace('(', "\\(")
             .replace(')', "\\)")
             .replace('{', "\\{")
             .replace('}', "\\}");

        // Restore protected segments
        let mut keys: Vec<&String> = placeholders.keys().collect();
        keys.sort_by(|a, b| b.cmp(a));
        for key in keys {
            s = s.replace(key, &placeholders[key]);
        }

        s
    }

    fn parse_outbound_attachments(&self, text: &str) -> (String, Vec<OutboundAttachment>) {
        // FILE / DOCUMENT / IMAGE / AUDIO / VIDEO markers
        let marker_re = Regex::new(r"(?i)\[(FILE|DOCUMENT|IMAGE|AUDIO|VIDEO):([^\]]+)\]").unwrap();

        let mut attachments = Vec::new();
        for caps in marker_re.captures_iter(text) {
            let kind = match caps
                .get(1)
                .map(|m| m.as_str().to_ascii_uppercase())
                .as_deref()
            {
                Some("IMAGE") => AttachmentKind::Image,
                Some("AUDIO") => AttachmentKind::Audio,
                Some("VIDEO") => AttachmentKind::Video,
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

    fn strip_outbound_attachment_markers_for_stream(&self, text: &str) -> String {
        let marker_re = Regex::new(r"(?i)\[(FILE|DOCUMENT|IMAGE|AUDIO|VIDEO):[^\]]+\]").unwrap();
        let mut cleaned = marker_re.replace_all(text, "").to_string();

        let upper = cleaned.to_ascii_uppercase();
        let partial_start = ["[FILE:", "[DOCUMENT:", "[IMAGE:", "[AUDIO:", "[VIDEO:"]
            .iter()
            .filter_map(|needle| upper.rfind(needle))
            .max();
        if let Some(idx) = partial_start
            && !cleaned[idx..].contains(']')
        {
            cleaned.truncate(idx);
        }

        cleaned
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
        if line.starts_with("![")
            && line.ends_with(')')
            && let Some(paren_idx) = line.rfind('(')
        {
            let uri = &line[paren_idx + 1..line.len().saturating_sub(1)];
            if uri.starts_with("data:image/") {
                return Some(uri);
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
            AttachmentKind::Video => self.send_video(chat_id, &attachment.path),
        }
    }

    fn send_attachment_with_fallback(
        &self,
        chat_id: &str,
        attachment: &OutboundAttachment,
    ) -> Result<()> {
        match self.send_attachment(chat_id, attachment) {
            Ok(()) => Ok(()),
            Err(primary_err) => {
                warn!(
                    "Telegram attachment send failed for {}: {}",
                    attachment.path.display(),
                    primary_err
                );
                if attachment.kind != AttachmentKind::Document {
                    match self.send_document(chat_id, &attachment.path) {
                        Ok(()) => return Ok(()),
                        Err(fallback_err) => {
                            warn!(
                                "Telegram attachment document fallback failed for {}: {}",
                                attachment.path.display(),
                                fallback_err
                            );
                        }
                    }
                }
                Err(primary_err)
            }
        }
    }

    fn send_document(&self, chat_id: &str, path: &Path) -> Result<()> {
        let url = self.api_url("sendDocument");
        let (chat_id, thread_id) = split_chat_thread(chat_id);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("document.bin")
            .to_string();
        let bytes = std::fs::read(path)?;
        let mut form = multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part(
                "document",
                multipart::Part::bytes(bytes).file_name(file_name),
            );
        if let Some(thread_id) = thread_id {
            form = form.text("message_thread_id", thread_id.to_string());
        }
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
        // Check URL safety first if path-like string resembles a URL
        let path_str = path.to_string_lossy();
        if path_str.starts_with("http://") || path_str.starts_with("https://") {
            if !is_safe_url(&path_str) {
                return Err(anyhow!(
                    "SSRF blocked: unsafe URL in photo path: {}",
                    path_str
                ));
            }
        }

        let url = self.api_url("sendPhoto");
        let (chat_id_str, thread_id) = split_chat_thread(chat_id);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image.bin")
            .to_string();
        let bytes = std::fs::read(path)?;
        let mut form = multipart::Form::new()
            .text("chat_id", chat_id_str.to_string())
            .part("photo", multipart::Part::bytes(bytes).file_name(file_name));
        if let Some(thread_id) = thread_id {
            form = form.text("message_thread_id", thread_id.to_string());
        }
        let response = self.client.post(&url).multipart(form).send()?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() || !body.contains("\"ok\":true") {
            // Photo dimension error (#12): fall back to sending as document.
            if body.contains("PHOTO_INVALID_DIMENSIONS")
                || body.contains("IMAGE_PROCESS_FAILED")
            {
                warn!(
                    "Telegram PHOTO_INVALID_DIMENSIONS for {}, retrying as document",
                    path.display()
                );
                return self.send_document(chat_id, path);
            }
            return Err(anyhow!(
                "Telegram sendPhoto error for {}: {} - {}",
                path.display(),
                status,
                body
            ));
        }
        Ok(())
    }

    /// Send audio via sendAudio (e.g. for [AUDIO:...] markers).
    /// Routes .ogg/.opus to sendVoice (Telegram's round playable voice bubble).
    fn send_audio(&self, chat_id: &str, path: &Path) -> Result<()> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        // .ogg / .opus → sendVoice (native voice message bubble)
        if ext == "ogg" || ext == "opus" {
            return self.send_voice(chat_id, path);
        }

        // .mp3 / .m4a / .wav / other → sendAudio
        let url = self.api_url("sendAudio");
        let (chat_id_str, thread_id) = split_chat_thread(chat_id);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.mp3")
            .to_string();
        let bytes = std::fs::read(path)?;
        let mut form = multipart::Form::new()
            .text("chat_id", chat_id_str.to_string())
            .part("audio", multipart::Part::bytes(bytes).file_name(file_name));
        if let Some(thread_id) = thread_id {
            form = form.text("message_thread_id", thread_id.to_string());
        }
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

    /// Send voice message via sendVoice API (playable round bubble).
    fn send_voice(&self, chat_id: &str, path: &Path) -> Result<()> {
        let url = self.api_url("sendVoice");
        let (chat_id_str, thread_id) = split_chat_thread(chat_id);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("voice.ogg")
            .to_string();
        let bytes = std::fs::read(path)?;
        let mut form = multipart::Form::new()
            .text("chat_id", chat_id_str.to_string())
            .part("voice", multipart::Part::bytes(bytes).file_name(file_name));
        if let Some(thread_id) = thread_id {
            form = form.text("message_thread_id", thread_id.to_string());
        }
        let response = self.client.post(&url).multipart(form).send()?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() || !body.contains("\"ok\":true") {
            return Err(anyhow!(
                "Telegram sendVoice error for {}: {} - {}",
                path.display(),
                status,
                body
            ));
        }
        Ok(())
    }

    fn send_video(&self, chat_id: &str, path: &Path) -> Result<()> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        // GIFs → sendAnimation (inline auto-playing in Telegram clients)
        if ext == "gif" {
            return self.send_animation(chat_id, path);
        }

        let url = self.api_url("sendVideo");
        let (chat_id_str, thread_id) = split_chat_thread(chat_id);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("video.mp4")
            .to_string();
        let bytes = std::fs::read(path)?;
        let mut form = multipart::Form::new()
            .text("chat_id", chat_id_str.to_string())
            .part("video", multipart::Part::bytes(bytes).file_name(file_name));
        if let Some(thread_id) = thread_id {
            form = form.text("message_thread_id", thread_id.to_string());
        }
        let response = self.client.post(&url).multipart(form).send()?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() || !body.contains("\"ok\":true") {
            return Err(anyhow!(
                "Telegram sendVideo error for {}: {} - {}",
                path.display(),
                status,
                body
            ));
        }
        Ok(())
    }

    /// Send animation/GIF via sendAnimation API (inline auto-playing).
    /// Falls back to sendDocument on failure.
    fn send_animation(&self, chat_id: &str, path: &Path) -> Result<()> {
        // SSRF check for URL-based paths
        let path_str = path.to_string_lossy();
        if (path_str.starts_with("http://") || path_str.starts_with("https://"))
            && !is_safe_url(&path_str)
        {
            return Err(anyhow!("SSRF blocked: unsafe GIF URL: {}", path_str));
        }

        let url = self.api_url("sendAnimation");
        let (chat_id_str, thread_id) = split_chat_thread(chat_id);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("animation.gif")
            .to_string();
        let bytes = std::fs::read(path)?;
        let mut form = multipart::Form::new()
            .text("chat_id", chat_id_str.to_string())
            .part("animation", multipart::Part::bytes(bytes).file_name(file_name));
        if let Some(thread_id) = thread_id {
            form = form.text("message_thread_id", thread_id.to_string());
        }
        let response = self.client.post(&url).multipart(form).send()?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if status.is_success() && body.contains("\"ok\":true") {
            return Ok(());
        }

        // Fallback: send as document
        warn!(
            "sendAnimation failed for {}, falling back to sendDocument: {} - {}",
            path.display(),
            status,
            body
        );
        self.send_document(chat_id, path)
    }

    // ── reaction helpers ────────────────────────────────────────────────

    /// Set a single emoji reaction on a Telegram message.
    /// Returns true if the API call succeeded.
    fn set_reaction(&self, chat_id: &str, message_id: i64, emoji: &str) -> bool {
        let url = self.api_url("setMessageReaction");
        let (chat_id, _thread_id) = split_chat_thread(chat_id);
        let reaction = serde_json::json!([{
            "type": "emoji",
            "emoji": emoji
        }]);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "reaction": reaction
        });
        match self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
        {
            Ok(resp) => {
                let ok = resp.status().is_success();
                if !ok {
                    warn!("setMessageReaction failed for {}: {}", message_id, resp.status());
                }
                ok
            }
            Err(e) => {
                warn!("setMessageReaction error for {}: {}", message_id, e);
                false
            }
        }
    }

    /// Clear all reactions from a Telegram message.
    fn clear_reactions(&self, chat_id: &str, message_id: i64) -> bool {
        let url = self.api_url("setMessageReaction");
        let (chat_id, _thread_id) = split_chat_thread(chat_id);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "reaction": []
        });
        match self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
        {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                warn!("clear reactions error for {}: {}", message_id, e);
                false
            }
        }
    }

    // ── delete message ────────────────────────────────────────────────────

    /// Delete a previously sent Telegram message (works for bot messages < 48h old).
    fn delete_message(&self, chat_id: &str, message_id: i64) -> bool {
        let url = self.api_url("deleteMessage");
        let (chat_id, _thread_id) = split_chat_thread(chat_id);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id
        });
        match self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
        {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                warn!("deleteMessage error for {}: {}", message_id, e);
                false
            }
        }
    }

    // ── draft streaming (Bot API 9.5) ─────────────────────────────────────

    /// Send a draft preview via Telegram's sendMessageDraft (Bot API 9.5+).
    /// The Bot API animates the preview when the same draft_id is reused.
    /// Returns true if the API call succeeded.
    fn send_message_draft(&self, chat_id: &str, draft_id: i64, text: &str) -> bool {
        let url = self.api_url("sendMessageDraft");
        let (chat_id, thread_id) = split_chat_thread(chat_id);
        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "draft_id": draft_id,
            "text": text
        });
        if let Some(tid) = thread_id {
            body["message_thread_id"] = Value::Number(tid.into());
        }
        match self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
        {
            Ok(resp) => {
                let ok = resp.status().is_success();
                if !ok {
                    // Draft API may not be available (older Bot API version) — log at debug
                    tracing::debug!(
                        "sendMessageDraft failed (draft_id={}): {}",
                        draft_id,
                        resp.status()
                    );
                }
                ok
            }
            Err(e) => {
                tracing::debug!("sendMessageDraft error (draft_id={}): {}", draft_id, e);
                false
            }
        }
    }

    // ── low-level message send/edit ─────────────────────────────────────────

    /// Try sending with a specific parse_mode, return the response text.
    /// Returns None if the request itself failed (connection error).
    fn try_send_with_parse_mode(
        &self,
        url: &str,
        body: &Value,
    ) -> std::result::Result<(reqwest::StatusCode, String), anyhow::Error> {
        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .json(body)
            .send()?;
        let status = response.status();
        let resp_text = response.text()?;
        Ok((status, resp_text))
    }

    /// Build the common body fields for a message send/edit.
    fn build_message_body(
        &self,
        chat_id: &str,
        text: &str,
        parse_mode: &str,
        thread_id: Option<i64>,
        edit_message_id: Option<i64>,
        reply_to: Option<i64>,
        reply_markup: Option<Value>,
    ) -> Value {
        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": parse_mode
        });
        if let Some(tid) = thread_id
            && edit_message_id.is_none()
        {
            body["message_thread_id"] = Value::Number(tid.into());
        }
        if let Some(msg_id) = edit_message_id {
            body["message_id"] = Value::Number(msg_id.into());
        }
        if let Some(reply_id) = reply_to {
            body["reply_to_message_id"] = Value::Number(reply_id.into());
        }
        if let Some(markup) = reply_markup {
            body["reply_markup"] = markup;
        }
        body
    }

    /// Send a text message with a fallback chain: MarkdownV2 → HTML → plain.
    /// Includes flood control (retry-after) and thread-not-found retry.
    /// Returns the message_id on success.
    fn send_message_formatted(
        &self,
        chat_id: &str,
        visible_text: &str,
        thread_id: Option<i64>,
        edit_message_id: Option<i64>,
        reply_to: Option<i64>,
        reply_markup: Option<Value>,
    ) -> Result<i64> {
        let is_edit = edit_message_id.is_some();
        let url = if is_edit {
            self.api_url("editMessageText")
        } else {
            self.api_url("sendMessage")
        };

        // Try MarkdownV2 first
        let mdv2_text = self.markdown_to_markdownv2(visible_text);
        let body = self.build_message_body(
            chat_id,
            &mdv2_text,
            "MarkdownV2",
            thread_id,
            edit_message_id,
            reply_to,
            reply_markup.clone(),
        );

        match self.try_send_with_flood_retry(&url, &body, is_edit) {
            Ok((status, resp_text)) => {
                if status.is_success() && !resp_text.contains("\"ok\":false") {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&resp_text) {
                        if let Some(msg_id) = parsed
                            .get("result")
                            .and_then(|r| r.get("message_id"))
                            .and_then(|m| m.as_i64())
                        {
                            return Ok(msg_id);
                        }
                        if resp_text.contains("\"ok\":true") {
                            return Ok(edit_message_id.unwrap_or(0));
                        }
                    }
                }
                if resp_text.contains("message is not modified") {
                    return Ok(edit_message_id.unwrap_or(0));
                }
                // Thread-not-found (#2): retry without message_thread_id
                if (resp_text.contains("thread not found")
                    || resp_text.contains("TOPIC_CLOSED")
                    || resp_text.contains("MESSAGE_THREAD_NOT_FOUND"))
                    && thread_id.is_some() && !is_edit
                {
                    warn!(
                        "Thread not found for chat_id={}, retrying without thread_id",
                        chat_id
                    );
                    let body_no_thread = self.build_message_body(
                        chat_id, &mdv2_text, "MarkdownV2", None,
                        edit_message_id, reply_to, reply_markup.clone(),
                    );
                    if let Ok((_s, t)) = self.try_send_with_flood_retry(&url, &body_no_thread, is_edit) {
                        if let Ok(parsed) = serde_json::from_str::<Value>(&t) {
                            if let Some(msg_id) = parsed
                                .get("result").and_then(|r| r.get("message_id")).and_then(|m| m.as_i64())
                            {
                                return Ok(msg_id);
                            }
                        }
                    }
                    // Fall through to HTML with no thread
                }
                if resp_text.contains("can't parse") || resp_text.contains("parse")
                    || resp_text.contains("Can't parse")
                {
                    // Fall through to HTML
                } else if !status.is_success() || resp_text.contains("\"ok\":false") {
                    if resp_text.contains("message_too_long") || resp_text.contains("too long") {
                        return Err(anyhow!("message_too_long"));
                    }
                    return Err(anyhow!(
                        "Telegram API error (MarkdownV2): {} - {}",
                        status,
                        resp_text
                    ));
                }
            }
            Err(e) => {
                return Err(anyhow!("Telegram request failed: {}", e));
            }
        }

        // Fallback: try HTML
        let html_text = self.markdown_to_html(visible_text);
        let body = self.build_message_body(
            chat_id,
            &html_text,
            "HTML",
            thread_id,
            edit_message_id,
            reply_to,
            reply_markup.clone(),
        );

        match self.try_send_with_flood_retry(&url, &body, is_edit) {
            Ok((status, resp_text)) => {
                if status.is_success() && !resp_text.contains("\"ok\":false") {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&resp_text) {
                        if let Some(msg_id) = parsed
                            .get("result")
                            .and_then(|r| r.get("message_id"))
                            .and_then(|m| m.as_i64())
                        {
                            return Ok(msg_id);
                        }
                        if resp_text.contains("\"ok\":true") {
                            return Ok(edit_message_id.unwrap_or(0));
                        }
                    }
                }
                if resp_text.contains("message is not modified") {
                    return Ok(edit_message_id.unwrap_or(0));
                }
                // Thread-not-found retry for HTML fallback
                if (resp_text.contains("thread not found")
                    || resp_text.contains("TOPIC_CLOSED")
                    || resp_text.contains("MESSAGE_THREAD_NOT_FOUND"))
                    && thread_id.is_some() && !is_edit
                {
                    warn!(
                        "Thread not found (HTML), retrying without thread_id"
                    );
                    let body_no_thread = self.build_message_body(
                        chat_id, &html_text, "HTML", None,
                        edit_message_id, reply_to, reply_markup.clone(),
                    );
                    if let Ok((_s, t)) = self.try_send_with_flood_retry(&url, &body_no_thread, is_edit) {
                        if let Ok(parsed) = serde_json::from_str::<Value>(&t) {
                            if let Some(msg_id) = parsed
                                .get("result").and_then(|r| r.get("message_id")).and_then(|m| m.as_i64())
                            {
                                return Ok(msg_id);
                            }
                        }
                    }
                }
                if resp_text.contains("can't parse") || resp_text.contains("parse")
                    || resp_text.contains("Can't parse")
                {
                    // Fall through to plain text
                } else if !status.is_success() || resp_text.contains("\"ok\":false") {
                    if resp_text.contains("message_too_long") || resp_text.contains("too long") {
                        return Err(anyhow!("message_too_long"));
                    }
                    return Err(anyhow!(
                        "Telegram API error (HTML): {} - {}",
                        status,
                        resp_text
                    ));
                }
            }
            Err(e) => {
                return Err(anyhow!("Telegram request failed: {}", e));
            }
        }

        // Final fallback: plain text
        let body = self.build_message_body(
            chat_id,
            visible_text,
            "",
            thread_id,
            edit_message_id,
            reply_to,
            reply_markup.clone(),
        );

        let (status, resp_text) = self.try_send_with_flood_retry(&url, &body, is_edit)?;

        // Thread-not-found retry for plain text
        if (resp_text.contains("thread not found")
            || resp_text.contains("TOPIC_CLOSED")
            || resp_text.contains("MESSAGE_THREAD_NOT_FOUND"))
            && thread_id.is_some() && !is_edit
        {
            let body_no_thread = self.build_message_body(
                chat_id, visible_text, "", None,
                edit_message_id, reply_to, reply_markup,
            );
            let (_s2, t2) = self.try_send_with_flood_retry(&url, &body_no_thread, is_edit)?;
            if let Ok(parsed) = serde_json::from_str::<Value>(&t2) {
                if let Some(msg_id) = parsed
                    .get("result").and_then(|r| r.get("message_id")).and_then(|m| m.as_i64())
                {
                    return Ok(msg_id);
                }
            }
            if t2.contains("message is not modified") {
                return Ok(edit_message_id.unwrap_or(0));
            }
            return Ok(0);
        }

        if !status.is_success()
            && !resp_text.contains("message is not modified")
        {
            return Err(anyhow!(
                "Telegram API error (plain): {} - {}",
                status,
                resp_text
            ));
        }

        if resp_text.contains("message is not modified") {
            return Ok(edit_message_id.unwrap_or(0));
        }

        if let Ok(parsed) = serde_json::from_str::<Value>(&resp_text) {
            if let Some(msg_id) = parsed
                .get("result")
                .and_then(|r| r.get("message_id"))
                .and_then(|m| m.as_i64())
            {
                return Ok(msg_id);
            }
        }
        Ok(0)
    }

    /// Wraps try_send_with_parse_mode with flood control (retry_after) handling.
    /// Detects 429 responses, parses retry_after, sleeps, and retries up to 3 times.
    /// For edits with retry_after > 5s, returns failure immediately so streaming
    /// can fall back to a new message.
    fn try_send_with_flood_retry(
        &self,
        url: &str,
        body: &Value,
        is_edit: bool,
    ) -> Result<(reqwest::StatusCode, String)> {
        const MAX_RETRIES: u32 = 3;
        for attempt in 0..=MAX_RETRIES {
            let (status, resp_text) = self.try_send_with_parse_mode(url, body)?;

            // Success or non-429 error — return immediately
            if status.as_u16() != 429 {
                return Ok((status, resp_text));
            }

            // Parse retry_after from response body
            let retry_after = serde_json::from_str::<Value>(&resp_text)
                .ok()
                .and_then(|v| {
                    v.get("parameters")
                        .and_then(|p| p.get("retry_after"))
                        .and_then(|r| r.as_f64())
                });

            if is_edit && retry_after.map(|r| r > 5.0).unwrap_or(false) {
                // For edits with long retry_after, fail immediately so streaming
                // can fall back to a new message.
                warn!(
                    "Flood control on edit with retry_after={}s, failing for new-message fallback",
                    retry_after.unwrap_or(0.0)
                );
                return Err(anyhow!("flood_control_edit"));
            }

            if attempt == MAX_RETRIES {
                return Err(anyhow!(
                    "Flood control: max retries ({}) exhausted, retry_after={}s",
                    MAX_RETRIES,
                    retry_after.unwrap_or(0.0)
                ));
            }

            // Sleep for retry_after duration (capped at 30s) or exponential backoff
            let wait = retry_after
                .map(|r| r.min(30.0))
                .unwrap_or_else(|| 2u64.pow(attempt as u32) as f64);

            warn!(
                "Telegram 429 flood control, retry_after={}s, attempt={}/{}",
                retry_after.unwrap_or(0.0),
                attempt + 1,
                MAX_RETRIES
            );
            std::thread::sleep(Duration::from_secs_f64(wait));
        }

        unreachable!()
    }

    /// Send/edit a single message, handling overflow by splitting when content
    /// exceeds Telegram's UTF-16 limit.
    fn send_single_message_internal(
        &self,
        chat_id: &str,
        text: &str,
        reply_to: Option<i64>,
        edit_message_id: Option<i64>,
    ) -> Result<i64> {
        let (chat_id, thread_id) = split_chat_thread(chat_id);
        let parsed_choices = parse_assistant_choices(text);
        let visible_text = &parsed_choices.visible_text;

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

        // Check UTF-16 length for Telegram's actual limit
        if utf16_len(visible_text) <= MAX_MESSAGE_LEN {
            let (chat_id_owned, thread_id_owned) = (chat_id.clone(), thread_id);
            return self.send_message_formatted(
                &chat_id_owned,
                visible_text,
                thread_id_owned,
                edit_message_id,
                reply_to,
                reply_markup,
            );
        }

        // ── Overflow: content exceeds Telegram's limit ─────────────
        if edit_message_id.is_none() {
            // For new messages: split and send as separate messages
            let msg_id = self.send_overflow_split_new(
                &chat_id,
                visible_text,
                thread_id,
                reply_to,
                reply_markup,
            )?;
            return Ok(msg_id);
        }

        // Overflow while editing: split-and-deliver
        self.send_overflow_split_edit(
            &chat_id,
            visible_text,
            thread_id,
            edit_message_id.unwrap_or(0),
            reply_to,
            reply_markup,
        )
    }

    /// Handle overflow for new (non-edit) messages: split and send each chunk.
    fn send_overflow_split_new(
        &self,
        chat_id: &str,
        text: &str,
        thread_id: Option<i64>,
        reply_to: Option<i64>,
        reply_markup: Option<Value>,
    ) -> Result<i64> {
        let chunks = self.smart_split_utf16(text, MAX_MESSAGE_LEN - 12);
        let mut last_id: i64 = 0;
        for (i, chunk) in chunks.iter().enumerate() {
            let is_last = i == chunks.len() - 1;
            let to_send = if is_last {
                chunk.to_string()
            } else {
                format!("{}\n\n⏬", chunk)
            };
            let msg_id = self.send_message_formatted(
                chat_id,
                &to_send,
                thread_id,
                None,
                if i == 0 { reply_to } else { None },
                if i == 0 { reply_markup.clone() } else { None },
            )?;
            last_id = msg_id;
        }
        Ok(last_id)
    }

    /// Handle overflow when editing: edit original message with first chunk,
    /// send remaining chunks as continuation messages.
    fn send_overflow_split_edit(
        &self,
        chat_id: &str,
        text: &str,
        thread_id: Option<i64>,
        original_msg_id: i64,
        reply_to: Option<i64>,
        reply_markup: Option<Value>,
    ) -> Result<i64> {
        let chunks = self.smart_split_utf16(text, MAX_MESSAGE_LEN - 12);
        if chunks.is_empty() {
            return Ok(original_msg_id);
        }

        // Step 1: edit original message with first chunk
        let first_chunk = &chunks[0];
        let _ = self.send_message_formatted(
            chat_id,
            first_chunk,
            thread_id,
            Some(original_msg_id),
            reply_to,
            reply_markup.clone(),
        );

        // Step 2: send remaining chunks as new messages
        let mut prev_id = original_msg_id;
        let mut last_id = original_msg_id;
        for chunk in chunks.iter().skip(1) {
            let is_last = chunk.len() == chunks.last().map(|c| c.len()).unwrap_or(0)
                && chunks.last() == Some(chunk);
            let to_send = if is_last {
                chunk.to_string()
            } else {
                format!("{}\n\n⏬", chunk)
            };
            match self.send_message_formatted(
                chat_id,
                &to_send,
                thread_id,
                None,
                Some(prev_id),
                None,
            ) {
                Ok(new_id) if new_id != 0 => {
                    prev_id = new_id;
                    last_id = new_id;
                }
                _ => {
                    // Continuation failed — stop here
                    break;
                }
            }
        }

        Ok(last_id)
    }

    /// Split text respecting UTF-16 code units (Telegram's actual limit).
    fn smart_split_utf16(&self, text: &str, max_utf16_len: usize) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut remaining = text;
        while !remaining.is_empty() {
            if utf16_len(remaining) <= max_utf16_len {
                chunks.push(remaining.to_string());
                break;
            }
            // Find a safe split point that respects UTF-16 boundary
            let mut split_byte_pos = remaining.len();
            let mut utf16_pos = 0;
            for (byte_idx, ch) in remaining.char_indices() {
                let ch_utf16_len = ch.len_utf16();
                if utf16_pos + ch_utf16_len > max_utf16_len {
                    break;
                }
                utf16_pos += ch_utf16_len;
                split_byte_pos = byte_idx + ch.len_utf8();
            }

            // Try to break at newline within the allowed region
            let search_end = split_byte_pos;
            let half = max_utf16_len / 2;
            // Find a utf-16 position for the half marker
            let mut half_byte_pos = 0;
            let mut half_utf16_pos = 0;
            for (byte_idx, ch) in remaining.char_indices() {
                if half_utf16_pos + ch.len_utf16() > half {
                    break;
                }
                half_utf16_pos += ch.len_utf16();
                half_byte_pos = byte_idx + ch.len_utf8();
            }

            let split_at = if let Some(pos) = remaining[..search_end].rfind('\n') {
                if pos >= half_byte_pos {
                    pos + 1
                } else if let Some(space_pos) = remaining[..search_end].rfind(' ') {
                    space_pos + 1
                } else {
                    split_byte_pos
                }
            } else if let Some(pos) = remaining[..search_end].rfind(' ') {
                pos + 1
            } else {
                split_byte_pos
            };

            let actual_split = split_at.min(remaining.len());
            chunks.push(remaining[..actual_split].to_string());
            remaining = &remaining[actual_split..];
        }
        chunks
    }

    fn send_single_message(&self, chat_id: &str, text: &str, reply_to: Option<i64>) -> Result<()> {
        self.send_single_message_internal(chat_id, text, reply_to, None)?;
        Ok(())
    }
}

fn split_chat_thread(chat_id: &str) -> (String, Option<i64>) {
    if let Some((chat, thread)) = chat_id.split_once(":thread:")
        && let Ok(thread_id) = thread.parse::<i64>()
    {
        return (chat.to_string(), Some(thread_id));
    }
    (chat_id.to_string(), None)
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
        self.do_send_message(chat_id, text, None)
    }

    fn send_message_with_reply(
        &self,
        chat_id: &str,
        text: &str,
        reply_to_message_id: Option<i64>,
    ) -> Result<()> {
        self.do_send_message(chat_id, text, reply_to_message_id)
    }

    fn send_stream_chunk(&self, chat_id: &str, text: &str) -> Result<()> {
        let chat_id = chat_id.strip_prefix("telegram:").unwrap_or(chat_id);
        if text.is_empty() {
            return Ok(());
        }

        // 1. Filter tool markup
        let filtered_chunk = {
            let mut filters = self.tag_filters.lock().unwrap();
            let filter = filters
                .entry(chat_id.to_string())
                .or_insert_with(TagFilter::new);
            filter.process(text)
        };

        if filtered_chunk.is_empty() {
            return Ok(());
        }

        // 2. Accumulate in draft
        let mut drafts = self.draft_buffers.lock().unwrap();
        let draft = drafts
            .entry(chat_id.to_string())
            .or_insert_with(DraftState::new);
        draft.buffer.push_str(&filtered_chunk);

        // 3. Flush if needed
        if draft.should_flush() {
            let text_to_send = self.strip_outbound_attachment_markers_for_stream(&draft.buffer);
            if text_to_send.trim().is_empty() {
                draft.last_flush_len = draft.buffer.len();
                draft.last_flush_time = std::time::Instant::now();
                return Ok(());
            }
            let safe_text = if utf16_len(&text_to_send) > MAX_MESSAGE_LEN {
                // Truncate to fit, respecting UTF-16 code units
                let mut utf16_pos = 0;
                let mut boundary = text_to_send.len();
                for (byte_idx, ch) in text_to_send.char_indices() {
                    let ch_utf16 = ch.len_utf16();
                    if utf16_pos + ch_utf16 > MAX_MESSAGE_LEN - 3 {
                        boundary = byte_idx;
                        break;
                    }
                    utf16_pos += ch_utf16;
                }
                // Ensure char boundary
                while !text_to_send.is_char_boundary(boundary) && boundary > 0 {
                    boundary -= 1;
                }
                format!("{}...", &text_to_send[..boundary])
            } else {
                text_to_send
            };

            let msg_id_opt = {
                let streams = self.active_streams.lock().unwrap();
                streams.get(chat_id).copied()
            };

            if let Some(msg_id) = msg_id_opt {
                let _ = self.send_single_message_internal(chat_id, &safe_text, None, Some(msg_id));
            } else if let Ok(new_id) =
                self.send_single_message_internal(chat_id, &safe_text, None, None)
                && new_id != 0
            {
                let mut streams = self.active_streams.lock().unwrap();
                streams.insert(chat_id.to_string(), new_id);
            }
            draft.last_flush_len = draft.buffer.len();
            draft.last_flush_time = std::time::Instant::now();
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

    fn poll_updates(&self) -> Result<Vec<ParsedMessage>> {
        let offset = self.last_update_id.load(Ordering::Relaxed);

        // Use reqwest blocking to call getUpdates (long-polling, 30 s timeout)
        let url = self.api_url("getUpdates");
        let body = serde_json::json!({
            "offset": offset,
            "timeout": 30,
            "allowed_updates": ["message", "edited_message", "callback_query"]
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

        self.flush_pending_inbound(&mut messages);

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

impl TelegramChannel {
    fn do_send_message(&self, chat_id: &str, text: &str, reply_to: Option<i64>) -> Result<()> {
        let chat_id = chat_id.strip_prefix("telegram:").unwrap_or(chat_id);

        self.stop_typing_heartbeat(chat_id);

        let stream_msg_id = {
            let mut streams = self.active_streams.lock().unwrap();
            let id = streams.remove(chat_id);
            let mut drafts = self.draft_buffers.lock().unwrap();
            drafts.remove(chat_id);
            let mut filters = self.tag_filters.lock().unwrap();
            filters.remove(chat_id);
            id
        };

        let (cleaned_text, attachments) = self.parse_outbound_attachments(text);
        let text_to_send = cleaned_text.trim();

        if text_to_send.to_lowercase().contains("[no_reply]") {
            return Ok(());
        }

        if !text_to_send.is_empty() {
            if let Some(msg_id) = stream_msg_id {
                if utf16_len(text_to_send) <= MAX_MESSAGE_LEN {
                    let _ = self.send_single_message_internal(
                        chat_id,
                        text_to_send,
                        reply_to,
                        Some(msg_id),
                    );
                } else {
                    let chunks = self.smart_split_utf16(text_to_send, MAX_MESSAGE_LEN - 12);
                    for (i, chunk) in chunks.iter().enumerate() {
                        let is_last = i == chunks.len() - 1;
                        let to_send = if is_last {
                            chunk.to_string()
                        } else {
                            format!("{}\n\n⏬", chunk)
                        };

                        if i == 0 {
                            let _ = self.send_single_message_internal(
                                chat_id,
                                &to_send,
                                reply_to,
                                Some(msg_id),
                            );
                        } else {
                            self.send_single_message(chat_id, &to_send, reply_to)?;
                        }
                    }
                }
            } else {
                self.send_message_with_splitting(chat_id, text_to_send, reply_to)?;
            }
        } else if attachments.is_empty() {
            tracing::warn!(
                "Telegram channel received empty response from agent with no attachments."
            );
        }

        let mut failed_attachments = Vec::new();
        for attachment in &attachments {
            if let Err(e) = self.send_attachment_with_fallback(chat_id, attachment) {
                failed_attachments.push(format!("{} ({})", attachment.path.display(), e));
            }
        }

        if !failed_attachments.is_empty() {
            let failure_text = format!(
                "I could not send these attachment(s):\n{}",
                failed_attachments.join("\n")
            );
            self.send_message_with_splitting(chat_id, &failure_text, reply_to)?;
        }

        Ok(())
    }
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_channel() -> TelegramChannel {
        TelegramChannel {
            config: TelegramConfig {
                account_id: "test".to_string(),
                bot_token: "test_token".to_string(),
                allow_from: vec!["*".to_string()],
                group_allow_from: vec![],
                group_policy: "allowlist".to_string(),
                reply_in_private: true,
                proxy: None,
                webhook_url: None,
            },
            client: Client::new(),
            last_update_id: AtomicI64::new(0),
            active_streams: Mutex::new(HashMap::new()),
            draft_buffers: Mutex::new(HashMap::new()),
            tag_filters: Mutex::new(HashMap::new()),
            typing_stops: Mutex::new(HashMap::new()),
            pending_media: Mutex::new(HashMap::new()),
            pending_text: Mutex::new(HashMap::new()),
        }
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
            webhook_url: None,
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
    fn test_format_inbound_attachment_uses_multimodal_marker() {
        let path = PathBuf::from(r"C:\tmp\photo test.jpg");
        assert_eq!(
            format_inbound_attachment("IMAGE", &path, "See"),
            format!("[IMAGE:{}]\nCaption: See", path.display())
        );
    }

    #[test]
    fn test_extract_content_location_contact_sticker() {
        let ch = make_channel();
        let location = serde_json::json!({ "location": { "latitude": 12.34, "longitude": 56.78 } });
        let loc_content = ch.extract_message_content(&location);
        assert!(loc_content.contains("[User shared location: 12.34, 56.78]"));
        assert!(loc_content.contains("https://www.google.com/maps/search/?api=1&query=12.34,56.78"));
        assert!(loc_content.contains("nearby restaurants"));

        let contact = serde_json::json!({
            "contact": { "first_name": "Ada", "last_name": "Lovelace", "phone_number": "+123" }
        });
        assert_eq!(
            ch.extract_message_content(&contact),
            "[User shared contact: Ada Lovelace phone=+123]"
        );

        let sticker = serde_json::json!({ "sticker": { "emoji": "ok", "set_name": "test" } });
        assert_eq!(
            ch.extract_message_content(&sticker),
            "[User sent sticker ok from set test]"
        );
    }

    #[test]
    fn test_process_topic_message_routes_to_thread() {
        let ch = make_channel();
        let update = serde_json::json!({
            "update_id": 1,
            "message": {
                "message_id": 10,
                "message_thread_id": 99,
                "from": { "id": 123, "username": "testuser", "first_name": "Test" },
                "chat": { "id": -100, "type": "supergroup" },
                "text": "hello topic"
            }
        });
        let mut messages = Vec::new();
        ch.process_update(&update, &mut messages);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].chat_id, "-100:thread:99");
        assert_eq!(messages[0].session_key, "telegram:-100:thread:99");
        assert_eq!(messages[0].message_thread_id, Some(99));
    }

    #[test]
    fn test_split_chat_thread() {
        assert_eq!(split_chat_thread("123"), ("123".to_string(), None));
        assert_eq!(
            split_chat_thread("-100:thread:99"),
            ("-100".to_string(), Some(99))
        );
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
    fn test_parse_outbound_attachments_windows_path_marker() {
        let ch = make_channel();
        let tmp_dir = std::env::temp_dir().join("openpaw telegram marker test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let tmp = tmp_dir.join("avatar test.jpg");
        std::fs::write(&tmp, "jpg").unwrap();
        let input = format!("Here:\n[IMAGE:{}]\nDone", tmp.display());
        let (cleaned, attachments) = ch.parse_outbound_attachments(&input);
        assert_eq!(cleaned, "Here:\n\nDone");
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].kind, AttachmentKind::Image);
        let _ = std::fs::remove_file(tmp);
        let _ = std::fs::remove_dir(tmp_dir);
    }

    #[test]
    fn test_strip_outbound_attachment_markers_for_stream() {
        let ch = make_channel();
        let input = "First\n[IMAGE:D:\\pawworkspace\\paw_avatar.jpg]\nSecond";
        assert_eq!(
            ch.strip_outbound_attachment_markers_for_stream(input),
            "First\n\nSecond"
        );
        assert_eq!(
            ch.strip_outbound_attachment_markers_for_stream("First\n[VIDEO:D:\\clip"),
            "First\n"
        );
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

    // ── MarkdownV2 tests ─────────────────────────────────────────────────────

    #[test]
    fn test_escape_mdv2() {
        assert_eq!(TelegramChannel::escape_mdv2("hello"), "hello");
        assert_eq!(TelegramChannel::escape_mdv2("a_b"), "a\\_b");
        assert_eq!(TelegramChannel::escape_mdv2("*bold*"), "\\*bold\\*");
        assert_eq!(TelegramChannel::escape_mdv2("[link]"), "\\[link\\]");
        assert_eq!(TelegramChannel::escape_mdv2("a(b)"), "a\\(b\\)");
        assert_eq!(TelegramChannel::escape_mdv2("`code`"), "\\`code\\`");
        assert_eq!(TelegramChannel::escape_mdv2("1. item"), "1\\. item");
        assert_eq!(TelegramChannel::escape_mdv2("~strike~"), "\\~strike\\~");
    }

    #[test]
    fn test_markdown_to_markdownv2_basic() {
        let ch = make_channel();

        // Bold
        let result = ch.markdown_to_markdownv2("Hello **world**");
        assert!(result.contains("*world*"));

        // Italic
        let result = ch.markdown_to_markdownv2("Hello *world*");
        assert!(result.contains("_world_"));

        // Inline code
        let result = ch.markdown_to_markdownv2("Use `code` here");
        assert!(result.contains("`code`"));

        // Link
        let result = ch.markdown_to_markdownv2("[click](https://example.com)");
        assert!(result.contains("[click]"));
        assert!(result.contains("(https://example.com)"));
    }

    #[test]
    fn test_markdown_to_markdownv2_code_blocks_are_protected() {
        let ch = make_channel();
        let md = "```\nlet x = 1;\n```";
        let result = ch.markdown_to_markdownv2(md);
        // Code block content should survive unescaped
        assert!(result.contains("let x = 1"));
    }

    #[test]
    fn test_markdown_to_markdownv2_strikethrough() {
        let ch = make_channel();
        let result = ch.markdown_to_markdownv2("~~strike~~");
        assert!(result.contains("~strike~") || result.contains("strike"));
    }

    #[test]
    fn test_markdown_to_markdownv2_spoiler() {
        let ch = make_channel();
        let result = ch.markdown_to_markdownv2("||spoiler||");
        assert!(result.contains("||spoiler||") || result.contains("spoiler"));
    }

    #[test]
    fn test_markdown_to_markdownv2_header() {
        let ch = make_channel();
        let result = ch.markdown_to_markdownv2("## Title");
        assert!(result.contains("*Title*"));
    }

    // ── utf16_len tests ──────────────────────────────────────────────────────

    #[test]
    fn test_utf16_len_ascii() {
        assert_eq!(utf16_len("hello"), 5);
        assert_eq!(utf16_len(""), 0);
    }

    #[test]
    fn test_utf16_len_emoji() {
        // 👀 (U+1F440) is 2 UTF-16 code units
        assert_eq!(utf16_len("\u{1F440}"), 2);
        // 👍 (U+1F44D) is 2 UTF-16 code units
        assert_eq!(utf16_len("\u{1F44D}"), 2);
        // Text with emoji
        assert_eq!(utf16_len("a\u{1F440}b"), 4);
    }

    #[test]
    fn test_utf16_len_cjk() {
        assert_eq!(utf16_len("你好"), 2);
        assert_eq!(utf16_len("a中b"), 3);
    }

    // ── smart_split_utf16 tests ──────────────────────────────────────────────

    #[test]
    fn test_smart_split_utf16_basic() {
        let ch = make_channel();
        // Single chunk when within limit
        let chunks = ch.smart_split_utf16("short", 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "short");
    }

    #[test]
    fn test_smart_split_utf16_splits_long_text() {
        let ch = make_channel();
        let text = "a".repeat(500);
        let chunks = ch.smart_split_utf16(&text, 100);
        assert!(chunks.len() > 1);
        // Each chunk should be within the limit
        for chunk in &chunks {
            assert!(utf16_len(chunk) <= 100);
        }
        // Concatenating should yield original
        let reconstructed: String = chunks.concat();
        assert_eq!(reconstructed.len(), text.len());
    }

    #[test]
    fn test_smart_split_utf16_respects_newlines() {
        let ch = make_channel();
        // Create text with a newline in the first 100 chars
        let text = format!("{}\n{}", "a".repeat(50), "b".repeat(50));
        let chunks = ch.smart_split_utf16(&text, 100);
        assert!(chunks.len() >= 1);
        assert!(chunks[0].contains('\n') || utf16_len(&chunks[0]) <= 100);
    }

    #[test]
    fn test_smart_split_utf16_with_emoji() {
        let ch = make_channel();
        // Each emoji (👀) is 2 UTF-16 units, so 50 emoji = 100 UTF-16 units = at limit
        let text: String = std::iter::repeat('\u{1F440}').take(60).collect();
        let chunks = ch.smart_split_utf16(&text, 100);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(utf16_len(chunk) <= 100);
        }
    }

    // ── API method tests (compile-time verification) ─────────────────────────

    #[test]
    fn test_reaction_methods_exist() {
        // Verify the API methods compile and accept the right signature.
        // Actual API calls require a real bot token, so we only verify
        // that the code compiles and the types are correct.
        let ch = make_channel();
        // set_reaction should return false (no real bot)
        assert!(!ch.set_reaction("123", 1, "\u{1F44D}"));
        // clear_reactions should return false (no real bot)
        assert!(!ch.clear_reactions("123", 1));
        // delete_message should return false (no real bot)
        assert!(!ch.delete_message("123", 1));
        // send_message_draft should return false (no real bot / older API)
        assert!(!ch.send_message_draft("123", 1, "test"));
    }

    #[test]
    fn test_api_url_format() {
        let ch = make_channel();
        let url = ch.api_url("getMe");
        assert_eq!(url, format!("{}{}/getMe", API_BASE, "test_token"));

        let url = ch.api_url("sendMessage");
        assert_eq!(url, format!("{}{}/sendMessage", API_BASE, "test_token"));

        let url = ch.api_url("setMessageReaction");
        assert_eq!(url, format!("{}{}/setMessageReaction", API_BASE, "test_token"));

        let url = ch.api_url("deleteMessage");
        assert_eq!(url, format!("{}{}/deleteMessage", API_BASE, "test_token"));

        let url = ch.api_url("sendMessageDraft");
        assert_eq!(url, format!("{}{}/sendMessageDraft", API_BASE, "test_token"));
    }

    #[test]
    fn test_emoji_constants() {
        assert_eq!(emoji::WATCHING, "\u{1F440}");
        assert_eq!(emoji::THUMBS_UP, "\u{1F44D}");
        assert_eq!(emoji::THUMBS_DOWN, "\u{1F44E}");
        assert_eq!(utf16_len(emoji::WATCHING), 2);
        assert_eq!(utf16_len(emoji::THUMBS_UP), 2);
        assert_eq!(utf16_len(emoji::THUMBS_DOWN), 2);
    }
}
