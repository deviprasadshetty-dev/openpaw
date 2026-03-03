use super::{Tool, ToolResult};
use anyhow::Result;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct MessageTool {
    pub default_channel: Option<String>,
    pub default_chat_id: Option<String>,
    pub sent_in_round: AtomicBool,
}

impl MessageTool {
    pub fn new() -> Self {
        Self {
            default_channel: None,
            default_chat_id: None,
            sent_in_round: AtomicBool::new(false),
        }
    }

    pub fn set_context(&mut self, channel: Option<String>, chat_id: Option<String>) {
        self.default_channel = channel;
        self.default_chat_id = chat_id;
        self.sent_in_round.store(false, Ordering::SeqCst);
    }

    pub fn has_message_been_sent(&self) -> bool {
        self.sent_in_round.load(Ordering::SeqCst)
    }
}

impl Tool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }

    fn description(&self) -> &str {
        "Send a message to a channel. If channel/chat_id are omitted, sends to the current conversation. Content supports attachment markers like [FILE:/abs/path], [DOCUMENT:/abs/path], [IMAGE:/abs/path] on marker-aware channels."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"content":{"type":"string","minLength":1,"description":"Message text to send"},"channel":{"type":"string","description":"Target channel (telegram, discord, slack, etc.). Defaults to current."},"chat_id":{"type":"string","description":"Target chat/room ID. Defaults to current."}},"required":["content"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolResult::fail("Missing required 'content' parameter")),
        };

        if content.trim().is_empty() {
            return Ok(ToolResult::fail("'content' must not be empty"));
        }

        let channel = args
            .get("channel")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| self.default_channel.as_deref().unwrap_or(""));

        if channel.is_empty() {
            return Ok(ToolResult::fail(
                "No channel specified and no default channel set",
            ));
        }

        let chat_id = args
            .get("chat_id")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| self.default_chat_id.as_deref().unwrap_or(""));

        if chat_id.is_empty() {
            return Ok(ToolResult::fail(
                "No chat_id specified and no default chat_id set",
            ));
        }

        // TODO: actually send over the event bus
        self.sent_in_round.store(true, Ordering::SeqCst);

        let result = format!(
            "Message sent to {}:{} ({} chars)",
            channel,
            chat_id,
            content.len()
        );
        Ok(ToolResult::ok(result))
    }
}
