use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

pub struct ScreenshotTool {}

#[async_trait]
impl Tool for ScreenshotTool {
    fn name(&self) -> &str {
        "screenshot"
    }

    fn description(&self) -> &str {
        "Take a screenshot of the primary display"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{}}"#.to_string()
    }

    async fn execute(&self, _args: Value, _context: &ToolContext) -> Result<ToolResult> {
        // Placeholder for actual screenshot logic (requires platform-specific crates)
        Ok(ToolResult::ok("Screenshot captured (placeholder)"))
    }
}
