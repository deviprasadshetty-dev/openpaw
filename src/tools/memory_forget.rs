use super::{Tool, ToolResult};
use crate::memory::MemoryStore;
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

pub struct MemoryForgetTool {
    pub memory: Arc<dyn MemoryStore>,
}

impl Tool for MemoryForgetTool {
    fn name(&self) -> &str {
        "memory_forget"
    }

    fn description(&self) -> &str {
        "Remove a memory by key. Use to delete outdated facts or sensitive data."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"key":{"type":"string","description":"The key of the memory to forget"}},"required":["key"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let key = match args.get("key").and_then(|v| v.as_str()) {
            Some(k) => k,
            None => return Ok(ToolResult::fail("Missing 'key' parameter")),
        };
        if key.is_empty() {
            return Ok(ToolResult::fail("'key' must not be empty"));
        }

        // Assume `forget` exists
        match self.memory.forget(key) {
            Ok(true) => Ok(ToolResult::ok(format!("Forgot memory: {}", key))),
            Ok(false) => Ok(ToolResult::ok(format!("No memory found with key: {}", key))),
            Err(e) => Ok(ToolResult::fail(format!(
                "Failed to forget memory '{}': {}",
                key, e
            ))),
        }
    }
}
