use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use std::sync::Arc;

pub struct MemoryListTool {
    pub memory: Arc<dyn crate::agent::memory_loader::Memory>,
}

#[async_trait]
impl Tool for MemoryListTool {
    fn name(&self) -> &str {
        "memory_list"
    }

    fn description(&self) -> &str {
        "List all keys stored in long-term memory"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{}}"#.to_string()
    }

    async fn execute(&self, _args: Value, context: &ToolContext) -> Result<ToolResult> {
        match self.memory.list(Some(&context.session_key)) {
            Ok(entries) => {
                if entries.is_empty() {
                    Ok(ToolResult::ok("Long-term memory is empty."))
                } else {
                    let keys: Vec<String> = entries.into_iter().map(|e| e.key).collect();
                    Ok(ToolResult::ok(format!(
                        "Heads up! I remember these items:\n- {}",
                        keys.join("\n- ")
                    )))
                }
            }
            Err(e) => Ok(ToolResult::fail(format!(
                "Failed to list memory keys: {}",
                e
            ))),
        }
    }
}
