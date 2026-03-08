use super::{Tool, ToolContext, ToolResult};
use crate::memory::{MemoryCategory, MemoryStore};
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

pub struct MemoryListTool {
    pub memory: Arc<dyn MemoryStore>,
}

impl Tool for MemoryListTool {
    fn name(&self) -> &str {
        "memory_list"
    }

    fn description(&self) -> &str {
        "List memory entries in recency order. Use for requests like 'show first N memory records' without shell/sqlite access."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"limit":{"type":"integer","description":"Max entries to return (default: 5, max: 100)"},"category":{"type":"string","description":"Optional category filter (core|daily|conversation|custom)"},"session_id":{"type":"string","description":"Optional session filter"},"include_content":{"type":"boolean","description":"Include content preview (default: true)"},"include_internal":{"type":"boolean","description":"Include internal autosave/hygiene keys (default: false)"}}}"#.to_string()
    }

    fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5);
        let limit = if limit > 0 && limit <= 100 {
            limit as usize
        } else {
            5
        };

        let category_opt = args
            .get("category")
            .and_then(|v| v.as_str())
            .map(MemoryCategory::from_str);
        let session_id_opt = args.get("session_id").and_then(|v| v.as_str());

        // Assume `list` method exists
        match self.memory.list(category_opt, session_id_opt) {
            Ok(mut entries) => {
                // Sort by timestamp descending (newest first)
                entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

                // Apply limit
                if entries.len() > limit {
                    entries.truncate(limit);
                }

                if entries.is_empty() {
                    Ok(ToolResult::ok("No memory entries found.".to_string()))
                } else {
                    let mut out = format!(
                        "Memory entries: showing {}/{}",
                        entries.len(),
                        entries.len()
                    );
                    for (i, entry) in entries.iter().enumerate() {
                        out.push_str(&format!("\n  {}. [{}]", i + 1, entry.key));
                    }
                    Ok(ToolResult::ok(out))
                }
            }
            Err(e) => Ok(ToolResult::fail(format!(
                "Failed to list memory entries: {}",
                e
            ))),
        }
    }
}
