/// Feature 5: Human-in-the-loop approval — main agent side.
///
/// The main agent uses this tool to relay the human's approve/deny decision
/// to a waiting subagent. The human says "approve 42" or "deny 42" and the
/// main agent calls this tool with the corresponding approval_id.
use super::{Tool, ToolContext, ToolResult};
use crate::approval::ApprovalManager;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct ApprovalRespondTool {
    pub approval_manager: Arc<ApprovalManager>,
}

#[async_trait]
impl Tool for ApprovalRespondTool {
    fn name(&self) -> &str {
        "approval_respond"
    }

    fn description(&self) -> &str {
        "Approve or deny a pending approval request from a subagent. \
         Use this when the user says something like 'approve 5' or 'deny 5'. \
         Also use 'list' mode to show all pending approvals waiting for a decision."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "approval_id": {
                    "type": "integer",
                    "description": "The approval ID to respond to (from the request_approval notification)"
                },
                "approved": {
                    "type": "boolean",
                    "description": "true = approve the action, false = deny it"
                },
                "list": {
                    "type": "boolean",
                    "description": "If true, list all pending approvals without responding to any"
                }
            }
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        // List mode
        if args.get("list").and_then(|v| v.as_bool()).unwrap_or(false) {
            let pending = self.approval_manager.list_pending();
            if pending.is_empty() {
                return Ok(ToolResult::ok("No pending approval requests."));
            }
            let mut out = format!("{} pending approval(s):\n\n", pending.len());
            for (id, question, label) in &pending {
                out.push_str(&format!(
                    "• **ID {}** (task: {})\n  {}\n\n",
                    id, label, question
                ));
            }
            return Ok(ToolResult::ok(out.trim_end().to_string()));
        }

        let approval_id = match args.get("approval_id").and_then(|v| v.as_u64()) {
            Some(id) => id,
            None => return Ok(ToolResult::fail(
                "Missing 'approval_id'. Provide an approval_id to respond to, or set list=true to see pending approvals."
            )),
        };

        let approved = match args.get("approved").and_then(|v| v.as_bool()) {
            Some(a) => a,
            None => return Ok(ToolResult::fail("Missing 'approved' (true or false)")),
        };

        match self.approval_manager.respond(approval_id, approved) {
            Ok(msg) => Ok(ToolResult::ok(msg)),
            Err(e) => Ok(ToolResult::fail(e.to_string())),
        }
    }
}
