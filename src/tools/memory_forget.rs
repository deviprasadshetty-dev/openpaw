use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use std::sync::Arc;

pub struct MemoryForgetTool {
    pub memory: Arc<dyn crate::agent::memory_loader::Memory>,
}

#[async_trait]
impl Tool for MemoryForgetTool {
    fn name(&self) -> &str {
        "memory_forget"
    }

    fn description(&self) -> &str {
        "Forget a key-value pair from long-term memory"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"key":{"type":"string","description":"The key to forget"}},"required":["key"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let key = match args.get("key").and_then(|v| v.as_str()) {
            Some(k) => k,
            None => return Ok(ToolResult::fail("Missing 'key' parameter")),
        };

        match self.memory.forget(key) {
            Ok(true) => Ok(ToolResult::ok(format!("Successfully forgot key: {}", key))),
            Ok(false) => Ok(ToolResult::fail(format!(
                "No memory found for key: {}",
                key
            ))),
            Err(e) => Ok(ToolResult::fail(format!("Failed to forget memory: {}", e))),
        }
    }
}
