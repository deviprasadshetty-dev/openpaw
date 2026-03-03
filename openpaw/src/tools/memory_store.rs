use super::{Tool, ToolResult};
use crate::memory::{MemoryCategory, MemoryStore};
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

pub struct MemoryStoreTool {
    pub memory: Arc<dyn MemoryStore>,
}

impl Tool for MemoryStoreTool {
    fn name(&self) -> &str {
        "memory_store"
    }

    fn description(&self) -> &str {
        "Store durable user facts, preferences, and decisions in long-term memory. Use category 'core' for stable facts, 'daily' for session notes, 'conversation' for important context only."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"key":{"type":"string","description":"Unique key for this memory"},"content":{"type":"string","description":"The information to remember"},"category":{"type":"string","enum":["core","daily","conversation"],"description":"Memory category"}},"required":["key","content"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let key = match args.get("key").and_then(|v| v.as_str()) {
            Some(k) => k,
            None => return Ok(ToolResult::fail("Missing 'key' parameter")),
        };
        if key.is_empty() { return Ok(ToolResult::fail("'key' must not be empty")); }

        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolResult::fail("Missing 'content' parameter")),
        };
        if content.is_empty() { return Ok(ToolResult::fail("'content' must not be empty")); }

        let category_str = args.get("category").and_then(|v| v.as_str()).unwrap_or("core");
        let category = MemoryCategory::from_str(category_str);

        match self.memory.store(key, content, category.clone(), None) {
            Ok(_) => {
                let msg = format!("Stored memory: {} ({})", key, category.to_string());
                Ok(ToolResult::ok(msg))
            }
            Err(e) => {
                let msg = format!("Failed to store memory '{}': {}", key, e);
                Ok(ToolResult::fail(msg))
            }
        }
    }
}
