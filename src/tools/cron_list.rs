use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use std::sync::Arc;

pub struct CronListTool {
    pub cron: Arc<crate::cron::CronScheduler>,
}

#[async_trait]
impl Tool for CronListTool {
    fn name(&self) -> &str {
        "cron_list"
    }

    fn description(&self) -> &str {
        "List all active cron jobs"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{}}"#.to_string()
    }

    async fn execute(&self, _args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let jobs = self.cron.list_jobs();

        if jobs.is_empty() {
            return Ok(ToolResult::ok("No active cron jobs"));
        }

        let mut output = String::from("Active Cron Jobs:\n");
        for job in jobs {
            output.push_str(&format!(
                "- {} (ID: {}): {} -> {}\n",
                job.name.as_deref().unwrap_or("unnamed"),
                job.id,
                job.expression,
                job.command
            ));
        }

        Ok(ToolResult::ok(output))
    }
}
