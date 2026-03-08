use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use serde_json::Value;

pub struct DelegateTool {}

impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "delegate"
    }

    fn description(&self) -> &str {
        "Delegate a subtask to a specialized agent. Use when a task benefits from a different model."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"agent":{"type":"string","minLength":1,"description":"Name of the agent to delegate to"},"prompt":{"type":"string","minLength":1,"description":"The task/prompt to send to the sub-agent"},"context":{"type":"string","description":"Optional context to prepend"}},"required":["agent","prompt"]}"#.to_string()
    }

    fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let agent_name = match args.get("agent").and_then(|v| v.as_str()) {
            Some(a) => a.trim(),
            None => return Ok(ToolResult::fail("Missing 'agent' parameter")),
        };

        if agent_name.is_empty() {
            return Ok(ToolResult::fail("'agent' parameter must not be empty"));
        }

        let prompt = match args.get("prompt").and_then(|v| v.as_str()) {
            Some(p) => p.trim(),
            None => return Ok(ToolResult::fail("Missing 'prompt' parameter")),
        };

        if prompt.is_empty() {
            return Ok(ToolResult::fail("'prompt' parameter must not be empty"));
        }

        let context = args.get("context").and_then(|v| v.as_str());

        let full_prompt = if let Some(ctx) = context {
            format!("Context: {}\n\n{}", ctx, prompt)
        } else {
            prompt.to_string()
        };

        // TODO: Interface with actual local sub-agent dispatcher/provider
        // Returning a placeholder for now since sync Tool::execute doesn't easily await async LLM providers without blocking
        let result = format!(
            "(Delegation to {} is mock completed)\nPrompt evaluated: {}",
            agent_name, full_prompt
        );
        Ok(ToolResult::ok(result))
    }
}
