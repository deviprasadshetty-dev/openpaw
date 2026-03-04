use crate::channels::root::{Channel, ParsedMessage};
use crate::config_types::TelegramConfig;
use crate::interactions::choices::parse_assistant_choices;
use anyhow::{Result, anyhow};
use reqwest::blocking::Client;
use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn};

const MAX_MESSAGE_LEN: usize = 4096;
const API_BASE: &str = "https://api.telegram.org/bot";

pub struct TelegramChannel {
    config: TelegramConfig,
    client: Client,
    last_update_id: AtomicI64,
    active_streams: Mutex<HashMap<String, i64>>,
}

impl TelegramChannel {
    pub fn new(config: TelegramConfig) -> Self {
        Self {
            client: Client::new(),
            config,
            last_update_id: AtomicI64::new(0),
            active_streams: Mutex::new(HashMap::new()),
        }
    }

    /// Build Telegram API URL for a method
    fn api_url(&self, method: &str) -> String {
        format!("{}{}/{}", API_BASE, self.config.bot_token, method)
    }

    /// Check if a user is allowed based on the allowlist
    fn is_user_allowed(&self, username: &str, user_id: &str) -> bool {
        // Empty allowlist = deny all
        if self.config.allow_from.is_empty() {
            return false;
        }

        for allowed in &self.config.allow_from {
            if allowed == "*" {
                return true;
            }
            // Strip leading "@" if present
            let check = if allowed.starts_with('@') {
                &allowed[1..]
            } else {
                allowed.as_str()
            };
            // Case-insensitive username comparison
            if check.eq_ignore_ascii_case(username) {
                return true;
            }
            // Exact user_id match
            if check == user_id {
                return true;
            }
        }
        false
    }

    /// Check if a user is allowed in a group chat
    fn is_group_user_allowed(&self, username: &str, user_id: &str) -> bool {
        match self.config.group_policy.as_str() {
            "open" => return true,
            "disabled" => return false,
            _ => {} // allowlist (default)
        }

        // Check group-specific allowlist first, then fall back to general allowlist
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

    /// Check authorization for a message
    fn is_authorized(&self, is_group: bool, username: &str, user_id: &str) -> bool {
        if is_group {
            self.is_group_user_allowed(username, user_id)
        } else {
            self.is_user_allowed(username, user_id)
        }
    }

    /// Send typing indicator to a chat (best-effort)
    fn send_typing_indicator(&self, chat_id: &str) {
        let url = self.api_url("sendChatAction");
        let body = serde_json::json!({
            "chat_id": chat_id,
            "action": "typing"
        });

        let _ = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send();
    }

    /// Process a single Telegram update and extract messages
    fn process_update(&self, update: &Value, messages: &mut Vec<ParsedMessage>) {
        // Extract update_id for offset tracking
        if let Some(uid) = update.get("update_id").and_then(|v| v.as_i64()) {
            let current = self.last_update_id.load(Ordering::Relaxed);
            if uid >= current {
                self.last_update_id.store(uid + 1, Ordering::Relaxed);
            }
        }

        // Handle callback queries (button clicks)
        if let Some(cbq) = update.get("callback_query") {
            self.process_callback_query(cbq, messages);
            return;
        }

        // Get message object
        let message = match update.get("message") {
            Some(m) => m,
            None => return,
        };

        // Extract sender info
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

        // Extract chat info
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

        // Authorization check
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

        // Extract content based on message type
        let content = self.extract_message_content(message);

        if content.is_empty() {
            return;
        }

        let sender_identity = if username != "unknown" {
            username.to_string()
        } else {
            user_id.clone()
        };

        let session_key = format!("telegram:{}", chat_id);

        let parsed = ParsedMessage {
            sender_id: sender_identity,
            chat_id,
            content,
            session_key,
            is_group,
            message_id,
            username: if username != "unknown" {
                Some(username.to_string())
            } else {
                None
            },
            first_name,
        };

        messages.push(parsed);
    }

    /// Extract content from different message types (text, voice, photo, document)
    fn extract_message_content(&self, message: &Value) -> String {
        // Try text first
        if let Some(text) = message.get("text").and_then(|v| v.as_str()) {
            return text.to_string();
        }

        // Handle caption-only messages (photo/video/document with caption but no text field)
        if let Some(caption) = message.get("caption").and_then(|v| v.as_str()) {
            return caption.to_string();
        }

        // Voice/audio messages - would need transcription integration
        if let Some(voice) = message.get("voice").or_else(|| message.get("audio")) {
            if let Some(file_id) = voice.get("file_id").and_then(|v| v.as_str()) {
                return format!("[Voice message: file_id={}]", file_id);
            }
        }

        // Photo messages
        if let Some(photos) = message.get("photo").and_then(|v| v.as_array()) {
            if let Some(last_photo) = photos.last() {
                if let Some(file_id) = last_photo.get("file_id").and_then(|v| v.as_str()) {
                    return format!("[Photo: file_id={}]", file_id);
                }
            }
        }

        // Document messages
        if let Some(doc) = message.get("document") {
            if let Some(file_id) = doc.get("file_id").and_then(|v| v.as_str()) {
                let file_name = doc
                    .get("file_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unnamed");
                return format!("[Document: {} file_id={}]", file_name, file_id);
            }
        }

        String::new()
    }

    /// Process callback queries (inline button clicks)
    fn process_callback_query(&self, cbq: &Value, messages: &mut Vec<ParsedMessage>) {
        let cb_id = cbq.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let cb_data = cbq.get("data").and_then(|v| v.as_str()).unwrap_or("");

        // Answer the callback query to remove loading state
        self.answer_callback_query(cb_id, None);

        // Extract user info
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

        // Get message info for context
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

        // Authorization check
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

        let session_key = format!("telegram:{}", chat_id);

        // Parse callback data format: "action:data"
        let content = format!("[Callback: {}]", cb_data);

        let parsed = ParsedMessage {
            sender_id: sender_identity,
            chat_id,
            content,
            session_key,
            is_group,
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
        };

        messages.push(parsed);
    }

    /// Answer a callback query to stop the loading indicator
    fn answer_callback_query(&self, cb_id: &str, text: Option<&str>) {
        let url = self.api_url("answerCallbackQuery");
        let mut body = serde_json::json!({
            "callback_query_id": cb_id
        });

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

    /// Send a message with smart splitting for long messages
    fn send_message_with_splitting(
        &self,
        chat_id: &str,
        text: &str,
        reply_to: Option<i64>,
    ) -> Result<()> {
        // Send typing indicator
        self.send_typing_indicator(chat_id);

        // Smart split at 4096 chars (Telegram limit)
        if text.len() <= MAX_MESSAGE_LEN {
            return self.send_single_message(chat_id, text, reply_to);
        }

        // Split into chunks
        let chunks = self.smart_split(text, MAX_MESSAGE_LEN - 12); // Leave room for continuation marker

        for (i, chunk) in chunks.iter().enumerate() {
            let is_last = i == chunks.len() - 1;
            let to_send = if is_last {
                chunk.to_string()
            } else {
                format!("{}\n\n⏬", chunk) // Down arrow to indicate continuation
            };

            self.send_single_message(chat_id, &to_send, if i == 0 { reply_to } else { None })?;
        }

        Ok(())
    }

    /// Smart split that prefers newlines and spaces
    fn smart_split(&self, text: &str, max_len: usize) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut remaining = text;

        while !remaining.is_empty() {
            if remaining.len() <= max_len {
                chunks.push(remaining.to_string());
                break;
            }

            let search_area = &remaining[..max_len];

            // Find split point - prefer newline in second half
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

    /// Convert Markdown bold (**text**) to HTML bold (<b>text</b>)
    fn markdown_bold_to_html(&self, text: &str) -> String {
        // Handle **bold** -> <b>bold</b>
        let mut result = text.to_string();
        let mut start = 0;

        while let Some(pos) = result[start..].find("**") {
            let abs_pos = start + pos;
            // Check if there's a closing **
            if let Some(close_pos) = result[abs_pos + 2..].find("**") {
                let abs_close = abs_pos + 2 + close_pos;
                // Replace ** with <b> and </b>
                result.replace_range(abs_pos..abs_pos + 2, "<b>");
                // Adjust close position since we changed the string
                let new_close = abs_close - 2 + 3; // -2 for **, +3 for <b>
                result.replace_range(new_close..new_close + 2, "</b>");
                start = new_close + 4; // Move past </b>
            } else {
                break;
            }
        }

        result
    }

    /// Send or edit a single message via the Telegram API
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

        // Parse nc_choices and convert markdown bold to HTML
        let parsed_choices = parse_assistant_choices(text);
        let html_text = self.markdown_bold_to_html(&parsed_choices.visible_text);

        // Build inline keyboard if choices are present
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

            // Arrange buttons in a row (up to 3 per row for better display)
            let mut inline_keyboard: Vec<Vec<Value>> = Vec::new();
            let chunk_size = 3;
            for chunk in keyboard_buttons.chunks(chunk_size) {
                inline_keyboard.push(chunk.to_vec());
            }

            Some(serde_json::json!({
                "inline_keyboard": inline_keyboard
            }))
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
            // Try without parse_mode if HTML failed
            if resp_text.contains("can't parse") || resp_text.contains("parse") {
                // Use the visible text without HTML tags and without nc_choices
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
                let out_id = parsed
                    .get("result")
                    .and_then(|r| r.get("message_id"))
                    .and_then(|m| m.as_i64())
                    .unwrap_or(0);
                return Ok(out_id);
            }
            if resp_text.contains("message is not modified") {
                return Ok(edit_message_id.unwrap_or(0));
            }
            return Err(anyhow!("Telegram API error: {} - {}", status, resp_text));
        }

        // Check for error in response body
        if resp_text.contains("\"ok\":false") {
            if resp_text.contains("message is not modified") {
                return Ok(edit_message_id.unwrap_or(0));
            }
            return Err(anyhow!("Telegram API returned error: {}", resp_text));
        }

        let parsed: Value = serde_json::from_str(&resp_text)?;
        let out_id = parsed
            .get("result")
            .and_then(|r| r.get("message_id"))
            .and_then(|m| m.as_i64())
            .unwrap_or(0);

        Ok(out_id)
    }

    fn send_single_message(&self, chat_id: &str, text: &str, reply_to: Option<i64>) -> Result<()> {
        self.send_single_message_internal(chat_id, text, reply_to, None)?;
        Ok(())
    }
}

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
        let edit_id = {
            let mut streams = self.active_streams.lock().unwrap();
            streams.remove(chat_id)
        };

        let text_to_send = if text.is_empty() { "..." } else { text };

        if let Some(msg_id) = edit_id {
            if text_to_send.len() <= MAX_MESSAGE_LEN {
                let _ =
                    self.send_single_message_internal(chat_id, text_to_send, None, Some(msg_id));
                return Ok(());
            }
        }

        self.send_message_with_splitting(chat_id, text_to_send, None)
    }

    fn send_stream_chunk(&self, chat_id: &str, text: &str) -> Result<()> {
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

    fn poll_updates(&self) -> Result<Vec<ParsedMessage>> {
        let url = self.api_url("getUpdates");

        let offset = self.last_update_id.load(Ordering::Relaxed);
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
            .timeout(std::time::Duration::from_secs(45))
            .send()?;

        let resp_text = response.text()?;

        let parsed: Value = serde_json::from_str(&resp_text)?;

        // Check if response is ok
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
            .timeout(std::time::Duration::from_secs(10))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smart_split() {
        let config = TelegramConfig {
            account_id: "test".to_string(),
            bot_token: "test_token".to_string(),
            allow_from: vec!["*".to_string()],
            group_allow_from: vec![],
            group_policy: "allowlist".to_string(),
            reply_in_private: true,
            proxy: None,
        };

        let channel = TelegramChannel::new(config);

        // Test basic split
        let text = "a".repeat(5000);
        let chunks = channel.smart_split(&text, 100);
        assert!(!chunks.is_empty());
        assert!(chunks[0].len() <= 100);

        // Test split at newline preference
        let text = "line1\nline2\nline3";
        let chunks = channel.smart_split(text, 20);
        assert_eq!(chunks.len(), 1); // Fits in one chunk
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

        let channel = TelegramChannel::new(config);

        assert!(channel.is_user_allowed("testuser", ""));
        assert!(channel.is_user_allowed("TestUser", "")); // Case insensitive
        assert!(channel.is_user_allowed("", "123456")); // By user ID
        assert!(!channel.is_user_allowed("otheruser", ""));
    }

    #[test]
    fn test_extract_content() {
        let config = TelegramConfig {
            account_id: "test".to_string(),
            bot_token: "test_token".to_string(),
            allow_from: vec!["*".to_string()],
            group_allow_from: vec![],
            group_policy: "allowlist".to_string(),
            reply_in_private: true,
            proxy: None,
        };

        let channel = TelegramChannel::new(config);

        // Test text message
        let msg = serde_json::json!({
            "text": "Hello world"
        });
        assert_eq!(channel.extract_message_content(&msg), "Hello world");

        // Test caption
        let msg = serde_json::json!({
            "caption": "Photo caption"
        });
        assert_eq!(channel.extract_message_content(&msg), "Photo caption");
    }
}
