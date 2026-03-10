use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

pub struct MessageTool {}

impl Default for MessageTool {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageTool {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl Tool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }

    fn description(&self) -> &str {
        "Send a message to a user or channel"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"message":{"type":"string","description":"The message to send"}},"required":["message"]}"#.to_string()
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolResult> {
        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => return Ok(ToolResult::fail("Missing 'message' parameter")),
        };

        // This would typically interface with the gateway to send messages
        tracing::info!("Agent message to {}: {}", context.channel, message);

        Ok(ToolResult::ok(format!("Sent message: {}", message)))
    }
}
