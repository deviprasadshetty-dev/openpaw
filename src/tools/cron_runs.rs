use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct CronRunsTool {
    pub cron: Arc<crate::cron::CronScheduler>,
}

#[async_trait]
impl Tool for CronRunsTool {
    fn name(&self) -> &str {
        "cron_runs"
    }

    fn description(&self) -> &str {
        "List recent executions of cron jobs"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{}}"#.to_string()
    }

    async fn execute(&self, _args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let runs = self.cron.runs.lock().unwrap().clone();

        if runs.is_empty() {
            return Ok(ToolResult::ok("No recent cron job executions"));
        }

        let mut output = String::from("Recent Cron Job Runs:\n");
        for run in runs.iter().rev().take(20) {
            output.push_str(&format!(
                "- Job: {} | Time: {} | Status: {} | Duration: {:?}ms\n",
                run.job_id, run.started_at_s, run.status, run.duration_ms
            ));
        }

        Ok(ToolResult::ok(output))
    }
}
