use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

/// BUG-CRON-4 FIX: Allows the agent to send a proactive message to the user's
/// Telegram chat without scheduling a cron job. Uses the global bus to publish
/// an OutboundMessage directly to the Telegram channel.
pub struct NotifyTelegramTool;

#[async_trait]
impl Tool for NotifyTelegramTool {
    fn name(&self) -> &str {
        "notify_telegram"
    }

    fn description(&self) -> &str {
        "Send a proactive message to the user's Telegram chat immediately. \
        Use this when you want to notify the user of something without waiting \
        for them to ask, e.g. after completing a background task, as a reminder, \
        or to share an important update. The message appears in the same chat \
        where the user is talking to you."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The message text to send to the user on Telegram (Markdown supported)"
                }
            },
            "required": ["message"]
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolResult> {
        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) if !m.trim().is_empty() => m.to_string(),
            _ => return Ok(ToolResult::fail("Missing or empty 'message' parameter")),
        };

        // Only send via Telegram channel. For other channels we just log.
        if context.channel != "telegram" && context.channel != "whatsapp" {
            return Ok(ToolResult::ok(format!(
                "Proactive notification would be sent (channel '{}' does not support direct push; message: {})",
                context.channel, message
            )));
        }

        let outbound = crate::bus::OutboundMessage {
            channel: context.channel.clone(),
            account_id: None, // outbound dispatcher resolves from channel config
            chat_id: context.chat_id.clone(),
            content: message.clone(),
            media: Vec::new(),
            stage: crate::streaming::OutboundStage::Final,
        };

        match crate::bus::global_bus() {
            Some(bus) => match bus.publish_outbound(outbound) {
                Ok(()) => Ok(ToolResult::ok(format!(
                    "Message sent to {} chat {}",
                    context.channel, context.chat_id
                ))),
                Err(e) => Ok(ToolResult::fail(format!(
                    "Failed to publish outbound message: {}",
                    e
                ))),
            },
            None => Ok(ToolResult::fail(
                "Global bus not initialized; cannot send proactive message",
            )),
        }
    }
}
