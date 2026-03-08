use super::{Tool, ToolContext, ToolResult};
use crate::tools::cron_utils::CronScheduler;
use anyhow::Result;
use serde_json::Value;

pub struct CronRemoveTool {}

impl Tool for CronRemoveTool {
    fn name(&self) -> &str {
        "cron_remove"
    }
    fn description(&self) -> &str {
        "Remove a scheduled cron job by its ID."
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"job_id":{"type":"string","description":"ID of the cron job to remove"}},"required":["job_id"]}"#.to_string()
    }

    fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let job_id = match args.get("job_id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id,
            _ => return Ok(ToolResult::fail("Missing required parameter: job_id")),
        };

        let mut scheduler = CronScheduler::new();
        if scheduler.remove_job(job_id) {
            Ok(ToolResult::ok(format!("Removed cron job {}", job_id)))
        } else {
            Ok(ToolResult::fail(format!("Job '{}' not found", job_id)))
        }
    }
}
