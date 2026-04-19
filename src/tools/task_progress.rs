/// Feature 4: Progress streaming from subagents.
///
/// A subagent calls `task_progress` to send an intermediate status update to the
/// origin channel without waiting for the full task to complete. The user sees
/// real-time heartbeats from long-running tasks.
use super::{Tool, ToolContext, ToolResult};
use crate::bus::Bus;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct TaskProgressTool {
    pub bus: Arc<Bus>,
}

#[async_trait]
impl Tool for TaskProgressTool {
    fn name(&self) -> &str {
        "task_progress"
    }

    fn description(&self) -> &str {
        "Send an intermediate progress update to the origin channel. Use this during long tasks \
         to keep the user informed without waiting for full completion. Call it after each \
         significant step (e.g. after fetching data, after writing a file, etc.)."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "Progress update message to send to the user (concise, 1-2 sentences)"
                },
                "step": {
                    "type": "integer",
                    "description": "Optional step number (e.g. 1 of 5)"
                },
                "total_steps": {
                    "type": "integer",
                    "description": "Optional total step count"
                }
            },
            "required": ["message"]
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolResult> {
        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) if !m.trim().is_empty() => m.trim(),
            _ => return Ok(ToolResult::fail("Missing or empty 'message' parameter")),
        };

        let step_info = match (
            args.get("step").and_then(|v| v.as_u64()),
            args.get("total_steps").and_then(|v| v.as_u64()),
        ) {
            (Some(s), Some(t)) => format!(" [{}/{}]", s, t),
            (Some(s), None) => format!(" [step {}]", s),
            _ => String::new(),
        };

        let full_msg = format!("⏳ Progress{}: {}", step_info, message);

        let outbound =
            crate::bus::make_outbound_chunk(&context.channel, &context.chat_id, &full_msg);
        let _ = self.bus.publish_outbound(outbound);

        Ok(ToolResult::ok(format!("Progress update sent: {}", message)))
    }
}
