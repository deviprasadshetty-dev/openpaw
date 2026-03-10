use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use std::sync::Arc;

pub struct MemoryStoreTool {
    pub memory: Arc<dyn crate::agent::memory_loader::Memory>,
}

#[async_trait]
impl Tool for MemoryStoreTool {
    fn name(&self) -> &str {
        "memory_store"
    }

    fn description(&self) -> &str {
        "Store a key-value pair in long-term memory"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"key":{"type":"string","description":"The key to store"},"value":{"type":"string","description":"The value to associate with the key"}},"required":["key","value"]}"#.to_string()
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolResult> {
        let key = match args.get("key").and_then(|v| v.as_str()) {
            Some(k) => k,
            None => return Ok(ToolResult::fail("Missing 'key' parameter")),
        };
        let value = match args.get("value").and_then(|v| v.as_str()) {
            Some(v) => v,
            None => return Ok(ToolResult::fail("Missing 'value' parameter")),
        };

        if let Err(e) = self.memory.store(key, value, Some(&context.session_key)) {
            return Ok(ToolResult::fail(format!("Failed to store memory: {}", e)));
        }

        Ok(ToolResult::ok(format!(
            "Successfully stored memory for key: {}",
            key
        )))
    }
}
