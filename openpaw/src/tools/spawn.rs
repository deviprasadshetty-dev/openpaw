use super::{Tool, ToolResult};
use anyhow::Result;
use serde_json::Value;

pub struct SpawnTool {
    pub default_channel: Option<String>,
    pub default_chat_id: Option<String>,
}

impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Spawn a background subagent to work on a task asynchronously. Returns a task ID immediately. Results are delivered as system messages when complete."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"task":{"type":"string","minLength":1,"description":"The task/prompt for the subagent"},"label":{"type":"string","description":"Optional human-readable label for tracking"}},"required":["task"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
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

        // let channel = self.default_channel.as_deref().unwrap_or("system");
        // let chat_id = self.default_chat_id.as_deref().unwrap_or("agent");

        // TODO: interface with an actual SubagentManager
        let task_id = 1; // mock task_id

        let msg = format!(
            "Subagent '{}' spawned with task_id={}. Results will be delivered as system messages.",
            label, task_id
        );
        Ok(ToolResult::ok(msg))
    }
}
