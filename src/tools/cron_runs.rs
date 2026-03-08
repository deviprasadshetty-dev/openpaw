use super::{Tool, ToolContext, ToolResult};
use crate::tools::cron_utils::CronScheduler;
use anyhow::Result;
use serde_json::Value;

pub struct CronRunsTool {}

impl Tool for CronRunsTool {
    fn name(&self) -> &str {
        "cron_runs"
    }
    fn description(&self) -> &str {
        "List recent execution history for a cron job."
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"job_id":{"type":"string","description":"ID of the cron job"},"limit":{"type":"integer","description":"Max runs to show (default 10)"}},"required":["job_id"]}"#.to_string()
    }

    fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let job_id = match arguments.get("job_id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id,
            _ => return Ok(ToolResult::fail("Missing 'job_id' parameter")),
        };

        let scheduler = CronScheduler::new();
        let job = match scheduler.jobs.get(job_id) {
            Some(j) => j,
            None => return Ok(ToolResult::fail(format!("Job '{}' not found", job_id))),
        };

        let last_run = job
            .last_run_secs
            .map(|s| s.to_string())
            .unwrap_or_else(|| "never".to_string());
        let last_status = job.last_status.as_deref().unwrap_or("pending");

        Ok(ToolResult::ok(format!(
            "Job {} | last_run: {} | last_status: {}\n(No run history detailed logging implemented in rust version yet)",
            job_id, last_run, last_status
        )))
    }
}
