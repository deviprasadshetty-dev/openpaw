use super::{Tool, ToolContext, ToolResult};
use crate::subagent::SubagentManager;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct TaskStatusTool {
    pub subagent_manager: Arc<SubagentManager>,
}

#[async_trait]
impl Tool for TaskStatusTool {
    fn name(&self) -> &str {
        "task_status"
    }

    fn description(&self) -> &str {
        "Show the status and captured result for a background subagent task by id. \
         If no task_id is provided, summarize currently queued and running tasks."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "integer",
                    "description": "Optional background task id to inspect"
                }
            }
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        if let Some(task_id) = args.get("task_id").and_then(|v| v.as_u64()) {
            return match self.subagent_manager.task_summary(task_id) {
                Some(summary) => Ok(ToolResult::ok(summary)),
                None => Ok(ToolResult::fail(format!("Task {} not found.", task_id))),
            };
        }

        let summaries = self.subagent_manager.list_task_summaries(20, false);
        if summaries.is_empty() {
            Ok(ToolResult::ok("No queued or running background tasks."))
        } else {
            Ok(ToolResult::ok(summaries.join("\n\n")))
        }
    }
}

pub struct TaskListTool {
    pub subagent_manager: Arc<SubagentManager>,
}

#[async_trait]
impl Tool for TaskListTool {
    fn name(&self) -> &str {
        "task_list"
    }

    fn description(&self) -> &str {
        "List background subagent tasks, including queued, running, and optionally completed tasks."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "include_completed": {
                    "type": "boolean",
                    "description": "Include completed, failed, and cancelled tasks. Defaults to false."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of tasks to return. Defaults to 20."
                }
            }
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let include_completed = args
            .get("include_completed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v.clamp(1, 100) as usize)
            .unwrap_or(20);

        let summaries = self
            .subagent_manager
            .list_task_summaries(limit, include_completed);
        if summaries.is_empty() {
            Ok(ToolResult::ok("No background tasks found."))
        } else {
            Ok(ToolResult::ok(summaries.join("\n\n")))
        }
    }
}
