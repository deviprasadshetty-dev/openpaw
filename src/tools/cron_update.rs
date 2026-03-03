use super::{Tool, ToolResult};
use crate::tools::cron_utils::CronScheduler;
use anyhow::Result;
use serde_json::Value;

pub struct CronUpdateTool {}

impl Tool for CronUpdateTool {
    fn name(&self) -> &str {
        "cron_update"
    }
    fn description(&self) -> &str {
        "Update a cron job: change expression, command, or enable/disable it."
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"job_id":{"type":"string","description":"ID of the cron job to update"},"expression":{"type":"string","description":"New cron expression"},"command":{"type":"string","description":"New command to execute"},"enabled":{"type":"boolean","description":"Enable or disable the job"}},"required":["job_id"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let job_id = match args.get("job_id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id,
            _ => return Ok(ToolResult::fail("Missing 'job_id' parameter")),
        };

        let expression = args.get("expression").and_then(|v| v.as_str());
        let command = args.get("command").and_then(|v| v.as_str());
        let enabled = args.get("enabled").and_then(|v| v.as_bool());

        if expression.is_none() && command.is_none() && enabled.is_none() {
            return Ok(ToolResult::fail(
                "Nothing to update — provide expression, command, or enabled",
            ));
        }

        let mut scheduler = CronScheduler::new();
        if let Some(job) = scheduler.jobs.get_mut(job_id) {
            if let Some(e) = expression {
                job.expression = e.to_string();
            }
            if let Some(c) = command {
                job.command = c.to_string();
            }
            if let Some(en) = enabled {
                job.paused = !en;
            }
            scheduler.save();
            Ok(ToolResult::ok(format!("Updated job {}", job_id)))
        } else {
            Ok(ToolResult::fail(format!("Job '{}' not found", job_id)))
        }
    }
}
