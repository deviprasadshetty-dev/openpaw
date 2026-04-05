use super::{Tool, ToolContext, ToolResult};
use crate::subagent::SubagentManager;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct TaskCancelTool {
    pub subagent_manager: Arc<SubagentManager>,
}

#[async_trait]
impl Tool for TaskCancelTool {
    fn name(&self) -> &str {
        "task_cancel"
    }

    fn description(&self) -> &str {
        "Cancel a running or queued subagent task by its task ID. The task is aborted immediately and the origin channel is notified."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"task_id":{"type":"integer","description":"The task ID to cancel, as returned by the spawn tool"}},"required":["task_id"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let task_id = match args.get("task_id").and_then(|v| v.as_u64()) {
            Some(id) => id,
            None => return Ok(ToolResult::fail("Missing or invalid 'task_id' parameter")),
        };

        match self.subagent_manager.cancel_task(task_id) {
            Ok(()) => Ok(ToolResult::ok(format!(
                "🚫 Task {} has been cancelled.",
                task_id
            ))),
            Err(e) => Ok(ToolResult::fail(format!(
                "Failed to cancel task {}: {}",
                task_id, e
            ))),
        }
    }
}
