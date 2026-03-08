use super::{Tool, ToolContext, ToolResult};
use crate::memory::MemoryStore;
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

pub struct MemoryRecallTool {
    pub memory: Arc<dyn MemoryStore>,
}

impl Tool for MemoryRecallTool {
    fn name(&self) -> &str {
        "memory_recall"
    }

    fn description(&self) -> &str {
        "Search long-term memory for relevant facts, preferences, or context."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"query":{"type":"string","description":"Keywords or phrase to search for in memory"},"limit":{"type":"integer","description":"Max results to return (default: 5)"}},"required":["query"]}"#.to_string()
    }

    fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let query = match arguments.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return Ok(ToolResult::fail("Missing 'query' parameter")),
        };
        if query.is_empty() {
            return Ok(ToolResult::fail("'query' must not be empty"));
        }

        let limit = arguments.get("limit").and_then(|v| v.as_u64()).unwrap_or(5);
        let limit = if limit > 0 && limit <= 100 {
            limit as usize
        } else {
            5
        };

        // Assume `recall` method exists
        match self.memory.recall(query, limit, None) {
            Ok(entries) => {
                if entries.is_empty() {
                    return Ok(ToolResult::ok(format!(
                        "No memories found matching: {}",
                        query
                    )));
                }

                let mut out = format!("Found {} memories:\n", entries.len());
                for (i, entry) in entries.iter().enumerate() {
                    out.push_str(&format!(
                        "{}. [{}] ({:?}): {}\n",
                        i + 1,
                        entry.key,
                        entry.category,
                        entry.content
                    ));
                }
                Ok(ToolResult::ok(out))
            }
            Err(e) => Ok(ToolResult::fail(format!(
                "Failed to recall memories for '{}': {}",
                query, e
            ))),
        }
    }
}
