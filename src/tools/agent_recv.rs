/// Feature 3: Inter-subagent messaging — receive side.
///
/// Read and consume messages from a named shared mailbox. Used together with
/// `agent_post` to pass data between pipeline stages.
use super::{Tool, ToolContext, ToolResult};
use crate::agent_mailbox::AgentMailbox;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct AgentRecvTool {
    pub mailbox: Arc<AgentMailbox>,
}

#[async_trait]
impl Tool for AgentRecvTool {
    fn name(&self) -> &str {
        "agent_recv"
    }

    fn description(&self) -> &str {
        "Read (and consume) messages from a named shared mailbox. Messages are returned \
         oldest-first and removed from the queue. Use peek=true to read without consuming. \
         Returns an empty list if the mailbox has no messages."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "mailbox": {
                    "type": "string",
                    "description": "Name of the mailbox to read from"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum messages to read (default: 10)",
                    "minimum": 1,
                    "maximum": 50
                },
                "peek": {
                    "type": "boolean",
                    "description": "If true, read messages without removing them (default: false)"
                }
            },
            "required": ["mailbox"]
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let mailbox = match args.get("mailbox").and_then(|v| v.as_str()) {
            Some(m) if !m.trim().is_empty() => m.trim(),
            _ => return Ok(ToolResult::fail("Missing or empty 'mailbox' parameter")),
        };

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n.min(50) as usize)
            .unwrap_or(10);

        let peek = args.get("peek").and_then(|v| v.as_bool()).unwrap_or(false);

        let messages = if peek {
            self.mailbox.peek(mailbox, limit)
        } else {
            self.mailbox.recv(mailbox, limit)
        };

        if messages.is_empty() {
            return Ok(ToolResult::ok(format!(
                "Mailbox '{}' is empty (no pending messages).",
                mailbox
            )));
        }

        let mode = if peek { "peek" } else { "consumed" };
        let mut out = format!(
            "Mailbox '{}' — {} message(s) {} ({}remaining: {}):\n\n",
            mailbox,
            messages.len(),
            mode,
            if peek { "" } else { "" },
            self.mailbox.count(mailbox),
        );

        for (i, msg) in messages.iter().enumerate() {
            out.push_str(&format!(
                "[{}/{}] from={} at={}:\n{}\n\n",
                i + 1,
                messages.len(),
                msg.from,
                msg.sent_at,
                msg.content.trim()
            ));
        }

        Ok(ToolResult::ok(out.trim_end().to_string()))
    }
}
