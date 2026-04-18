use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }
    pub fn fail(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

#[derive(Clone)]
pub struct ToolContext {
    pub channel: String,
    pub sender_id: String,
    pub chat_id: String,
    pub session_key: String,
    pub task_kind: Option<String>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_json(&self) -> String;
    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult>;

    /// Optional method for tools that are deterministic and side-effect free,
    /// enabling short-term caching of their results.
    fn cacheable(&self) -> bool {
        false
    }
}
