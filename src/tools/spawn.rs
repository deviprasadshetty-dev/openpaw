use super::{Tool, ToolContext, ToolResult};
use crate::subagent::SubagentManager;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct SpawnTool {
    pub subagent_manager: Arc<SubagentManager>,
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Spawn a background subagent to work on a task asynchronously. Returns a task ID immediately. Results are delivered as system messages when complete."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"task":{"type":"string","minLength":1,"description":"The task/prompt for the subagent"},"label":{"type":"string","description":"Optional human-readable label for tracking"},"agent":{"type":"string","description":"Optional named agent profile from config's agents list to use as persona"},"origin_channel":{"type":"string","description":"Internal: channel to report back to"},"origin_chat_id":{"type":"string","description":"Internal: chat ID to report back to"}},"required":["task"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let task = match args.get("task").and_then(|v| v.as_str()) {
            Some(t) => t.trim(),
            None => return Ok(ToolResult::fail("Missing 'task' parameter")),
        };

        if task.is_empty() {
            return Ok(ToolResult::fail("'task' must not be empty"));
        }

        let label = args
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("subagent");

        let agent_name = args.get("agent").and_then(|v| v.as_str());

        let origin_channel = args
            .get("origin_channel")
            .and_then(|v| v.as_str())
            .unwrap_or("system");

        let origin_chat_id = args
            .get("origin_chat_id")
            .and_then(|v| v.as_str())
            .unwrap_or("agent");

        match self.subagent_manager.spawn_with_agent(
            task,
            label,
            origin_channel,
            origin_chat_id,
            agent_name,
        ) {
            Ok(task_id) => {
                let agent_str = agent_name.unwrap_or("default");
                let msg = format!(
                    "✅ Subagent '{}' (profile: {}) spawned successfully (Task ID: {}). It will report back to {}/{} when done.",
                    label, agent_str, task_id, origin_channel, origin_chat_id
                );
                Ok(ToolResult::ok(msg))
            }
            Err(e) => Ok(ToolResult::fail(format!("Failed to spawn subagent: {}", e))),
        }
    }
}
