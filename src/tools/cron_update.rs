use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct CronUpdateTool {
    pub cron: Arc<crate::cron::CronScheduler>,
}

#[async_trait]
impl Tool for CronUpdateTool {
    fn name(&self) -> &str {
        "cron_update"
    }

    fn description(&self) -> &str {
        "Update an existing cron job"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"id":{"type":"string","description":"ID of the job to update"},"expression":{"type":"string","description":"New cron expression (optional)"},"command":{"type":"string","description":"New command (optional)"}},"required":["id"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(i) => i,
            None => return Ok(ToolResult::fail("Missing required 'id' parameter")),
        };

        let expression = args.get("expression").and_then(|v| v.as_str());
        let command = args.get("command").and_then(|v| v.as_str());

        let mut jobs = self.cron.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(id) {
            if let Some(exp) = expression {
                job.expression = exp.to_string();
            }
            if let Some(cmd) = command {
                job.command = cmd.to_string();
            }
            Ok(ToolResult::ok(format!("Updated cron job {}", id)))
        } else {
            Ok(ToolResult::fail(format!("Cron job {} not found", id)))
        }
    }
}
