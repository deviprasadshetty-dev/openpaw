use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct CronRemoveTool {
    pub cron: Arc<crate::cron::CronScheduler>,
}

#[async_trait]
impl Tool for CronRemoveTool {
    fn name(&self) -> &str {
        "cron_remove"
    }

    fn description(&self) -> &str {
        "Remove a cron job by ID"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"id":{"type":"string","description":"ID of the job to remove"}},"required":["id"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(i) => i,
            None => return Ok(ToolResult::fail("Missing required 'id' parameter")),
        };

        if self.cron.remove_job(id).is_some() {
            Ok(ToolResult::ok(format!("Removed cron job {}", id)))
        } else {
            Ok(ToolResult::fail(format!("Cron job {} not found", id)))
        }
    }
}
