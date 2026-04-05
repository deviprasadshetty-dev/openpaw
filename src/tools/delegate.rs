use super::{Tool, ToolContext, ToolResult};
use crate::subagent::SubagentManager;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct DelegateTool {
    pub subagent_manager: Arc<SubagentManager>,
}

#[async_trait]
impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "delegate"
    }

    fn description(&self) -> &str {
        "Delegate a task to a named agent profile. Spawns a background subagent using the specified agent's model, provider, and system prompt. Returns a task ID immediately; results are delivered as a system message when complete."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"agent_id":{"type":"string","description":"Named agent profile from config's agents list (e.g. \"researcher\", \"coder\")"},"task":{"type":"string","minLength":1,"description":"The task/prompt for the agent"},"label":{"type":"string","description":"Optional human-readable label for tracking"}},"required":["agent_id","task"]}"#.to_string()
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolResult> {
        let agent_id = match args.get("agent_id").and_then(|v| v.as_str()) {
            Some(id) => id.trim(),
            None => return Ok(ToolResult::fail("Missing 'agent_id' parameter")),
        };

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
            .unwrap_or(agent_id);

        match self.subagent_manager.spawn_with_agent(
            task,
            label,
            &context.channel,
            &context.chat_id,
            Some(agent_id),
        ) {
            Ok(task_id) => Ok(ToolResult::ok(format!(
                "✅ Delegated to agent '{}' as task {} (label: '{}').",
                agent_id, task_id, label
            ))),
            Err(e) => Ok(ToolResult::fail(format!(
                "Failed to delegate to agent '{}': {}",
                agent_id, e
            ))),
        }
    }
}
