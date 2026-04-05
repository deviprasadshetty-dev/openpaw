/// Feature 5: Human-in-the-loop approval — subagent side.
///
/// A subagent calls `request_approval` before a destructive or sensitive action.
/// The tool pauses execution, sends the question to the origin channel, and waits
/// for the human (via `approval_respond`) to approve or deny.
use super::{Tool, ToolContext, ToolResult};
use crate::approval::ApprovalManager;
use crate::bus::Bus;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

pub struct RequestApprovalTool {
    pub approval_manager: Arc<ApprovalManager>,
    pub bus: Arc<Bus>,
}

#[async_trait]
impl Tool for RequestApprovalTool {
    fn name(&self) -> &str {
        "request_approval"
    }

    fn description(&self) -> &str {
        "Pause and ask the human to approve or deny an action before proceeding. \
         Use this before any irreversible or destructive operations (deleting files, \
         sending messages, modifying critical configs, etc.). \
         The tool suspends until the human responds or the timeout expires. \
         If timed out or denied, returns a denial result — do NOT proceed with the action."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "Clear description of what action you want to take, and why. Include any relevant details (file paths, recipients, etc.) so the human can make an informed decision."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "How long to wait for a response in seconds (default: 300 = 5 minutes, max: 3600 = 1 hour)",
                    "minimum": 30,
                    "maximum": 3600
                }
            },
            "required": ["question"]
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolResult> {
        let question = match args.get("question").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim().to_string(),
            _ => return Ok(ToolResult::fail("Missing or empty 'question' parameter")),
        };

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(300)
            .min(3600);

        let task_label = context.session_key.clone();
        let (approval_id, receiver) = self.approval_manager.register(&question, &task_label);

        // Notify the origin channel
        let notice = format!(
            "🔐 **Approval Required** (ID: {})\n\n{}\n\n\
             ➜ Reply `approve {}` to allow or `deny {}` to cancel.\n\
             _(Waiting up to {} seconds)_",
            approval_id, question, approval_id, approval_id, timeout_secs
        );
        let outbound = crate::bus::make_outbound(&context.channel, &context.chat_id, &notice);
        let _ = self.bus.publish_outbound(outbound);

        // Wait for response or timeout
        match tokio::time::timeout(Duration::from_secs(timeout_secs), receiver).await {
            Ok(Ok(true)) => Ok(ToolResult::ok(format!(
                "✅ Approved (ID: {}). You may proceed with the action.",
                approval_id
            ))),
            Ok(Ok(false)) => Ok(ToolResult::ok(format!(
                "🚫 Denied (ID: {}). Do NOT proceed with the action — the human said no.",
                approval_id
            ))),
            Ok(Err(_)) => {
                // Sender dropped (shouldn't happen normally)
                Ok(ToolResult::ok(format!(
                    "⚠️ Approval {} was cancelled internally. Treat as denied.",
                    approval_id
                )))
            }
            Err(_) => {
                // Timeout — clean up the pending entry
                let _ = self.approval_manager.respond(approval_id, false);
                Ok(ToolResult::ok(format!(
                    "⏱️ Approval {} timed out after {} seconds. Treat as denied — do NOT proceed.",
                    approval_id, timeout_secs
                )))
            }
        }
    }
}
