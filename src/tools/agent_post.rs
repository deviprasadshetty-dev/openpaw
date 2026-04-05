/// Feature 3: Inter-subagent messaging — post side.
///
/// Post a message to a named shared mailbox so another agent (or the main agent)
/// can read it with `agent_recv`. Enables researcher → coder → reviewer pipelines.
use super::{Tool, ToolContext, ToolResult};
use crate::agent_mailbox::AgentMailbox;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct AgentPostTool {
    pub mailbox: Arc<AgentMailbox>,
}

#[async_trait]
impl Tool for AgentPostTool {
    fn name(&self) -> &str {
        "agent_post"
    }

    fn description(&self) -> &str {
        "Post a message to a named shared mailbox. Another agent (or the main agent) can read \
         it with agent_recv. Use this to pass results between pipeline stages — e.g. a researcher \
         posts findings, then a coder reads them."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "mailbox": {
                    "type": "string",
                    "description": "Name of the mailbox to post to (e.g. \"research_results\", \"review_queue\")"
                },
                "message": {
                    "type": "string",
                    "description": "Content to post"
                }
            },
            "required": ["mailbox", "message"]
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolResult> {
        let mailbox = match args.get("mailbox").and_then(|v| v.as_str()) {
            Some(m) if !m.trim().is_empty() => m.trim(),
            _ => return Ok(ToolResult::fail("Missing or empty 'mailbox' parameter")),
        };

        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => return Ok(ToolResult::fail("Missing 'message' parameter")),
        };

        let from = if context.sender_id.is_empty() {
            "agent".to_string()
        } else {
            context.sender_id.clone()
        };

        self.mailbox.post(mailbox, &from, message);

        Ok(ToolResult::ok(format!(
            "✉️ Posted to mailbox '{}' ({} chars). {} message(s) now pending.",
            mailbox,
            message.len(),
            self.mailbox.count(mailbox),
        )))
    }
}
