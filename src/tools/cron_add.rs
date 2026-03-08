use super::{Tool, ToolContext, ToolResult};
use crate::tools::cron_utils::CronScheduler;
use anyhow::Result;
use serde_json::Value;

pub struct CronAddTool {}

impl Tool for CronAddTool {
    fn name(&self) -> &str {
        "cron_add"
    }
    fn description(&self) -> &str {
        "Create a scheduled cron job. Provide either 'expression' (cron syntax) or 'delay' (e.g. '30m', '2h') plus 'command'."
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"expression":{"type":"string","description":"Cron expression (e.g. '*/5 * * * *')"},"delay":{"type":"string","description":"Delay for one-shot tasks (e.g. '30m', '2h')"},"command":{"type":"string","description":"Shell command to execute"},"name":{"type":"string","description":"Optional job name"}},"required":["command"]}"#.to_string()
    }

    fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolResult> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolResult::fail("Missing required 'command' parameter")),
        };

        let expression = args.get("expression").and_then(|v| v.as_str());
        let delay = args.get("delay").and_then(|v| v.as_str());

        if expression.is_none() && delay.is_none() {
            return Ok(ToolResult::fail(
                "Missing schedule: provide either 'expression' (cron syntax) or 'delay' (e.g. '30m')",
            ));
        }

        let mut scheduler = CronScheduler::new();
        match scheduler.add_job(
            expression.unwrap_or(""),
            command,
            delay,
            &context.channel,
            &context.chat_id,
            &context.session_key,
        ) {
            Ok(job) => Ok(ToolResult::ok(format!(
                "Created cron job {}: {} -> {}",
                job.id, job.expression, job.command
            ))),
            Err(e) => Ok(ToolResult::fail(format!("Failed to create job: {}", e))),
        }
    }
}
