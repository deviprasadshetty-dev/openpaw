use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use std::sync::Arc;

pub struct MemoryRecallTool {
    pub memory: Arc<dyn crate::agent::memory_loader::Memory>,
}

#[async_trait]
impl Tool for MemoryRecallTool {
    fn name(&self) -> &str {
        "memory_recall"
    }

    fn description(&self) -> &str {
        "Recall a value from long-term memory by key"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"key":{"type":"string","description":"The key to recall"}},"required":["key"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let key = match args.get("key").and_then(|v| v.as_str()) {
            Some(k) => k,
            None => return Ok(ToolResult::fail("Missing 'key' parameter")),
        };

        match self.memory.get(key) {
            Ok(Some(entry)) => Ok(ToolResult::ok(format!(
                "Key: {}\nValue: {}",
                key, entry.content
            ))),
            Ok(None) => Ok(ToolResult::fail(format!(
                "No memory found for key: {}",
                key
            ))),
            Err(e) => Ok(ToolResult::fail(format!("Failed to recall memory: {}", e))),
        }
    }
}
