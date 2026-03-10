use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct CronRunTool {
    pub cron: Arc<crate::cron::CronScheduler>,
}

#[async_trait]
impl Tool for CronRunTool {
    fn name(&self) -> &str {
        "cron_run"
    }

    fn description(&self) -> &str {
        "Trigger a cron job to run immediately"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"id":{"type":"string","description":"ID of the job to run"}},"required":["id"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(i) => i,
            None => return Ok(ToolResult::fail("Missing required 'id' parameter")),
        };

        if let Err(e) = self.cron.run_job(id).await {
            Ok(ToolResult::fail(format!("Failed to trigger cron job {}: {}", id, e)))
        } else {
            Ok(ToolResult::ok(format!("Triggered cron job {}", id)))
        }
    }
}
