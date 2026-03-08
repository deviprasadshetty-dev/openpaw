use super::{Tool, ToolContext, ToolResult};
use crate::tools::cron_utils::CronScheduler;
use anyhow::Result;
use serde_json::Value;

pub struct CronListTool {}

impl Tool for CronListTool {
    fn name(&self) -> &str {
        "cron_list"
    }
    fn description(&self) -> &str {
        "List all scheduled cron jobs with their status and next run time."
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{}}"#.to_string()
    }

    fn execute(&self, _args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let scheduler = CronScheduler::new();
        let jobs = scheduler.list_jobs();

        if jobs.is_empty() {
            return Ok(ToolResult::ok("No scheduled cron jobs."));
        }

        let mut out = String::new();
        for job in jobs {
            let status = if job.paused { "paused" } else { "enabled" };
            out.push_str(&format!(
                "- {} | {} | {} | next: {} | cmd: {}\n",
                job.id, job.expression, status, job.next_run_secs, job.command
            ));
        }
        Ok(ToolResult::ok(out))
    }
}
