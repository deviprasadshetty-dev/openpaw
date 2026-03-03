use anyhow::Result;
use serde_json::Value;

pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false }
    }
    pub fn fail(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true }
    }
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_json(&self) -> String;
    fn execute(&self, arguments: Value) -> Result<ToolResult>;
}
