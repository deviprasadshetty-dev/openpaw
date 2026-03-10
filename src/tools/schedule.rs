use super::{Tool, ToolContext, ToolResult};
use crate::tools::cron_utils::CronScheduler;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

pub struct ScheduleTool {}

#[async_trait]
impl Tool for ScheduleTool {
    fn name(&self) -> &str {
        "schedule"
    }

    fn description(&self) -> &str {
        "Manage scheduled tasks. Actions: create/add/once/list/get/cancel/remove/pause/resume"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"action":{"type":"string","enum":["create","add","once","list","get","cancel","remove","pause","resume"],"description":"Action to perform"},"expression":{"type":"string","description":"Cron expression for recurring tasks"},"delay":{"type":"string","description":"Delay for one-shot tasks (e.g. '30m', '2h')"},"command":{"type":"string","description":"Shell command to execute"},"id":{"type":"string","description":"Task ID"}},"required":["action"]}"#.to_string()
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolResult> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return Ok(ToolResult::fail("Missing 'action' parameter")),
        };

        let mut scheduler = CronScheduler::new();

        match action {
            "list" => {
                let jobs = scheduler.list_jobs();
                if jobs.is_empty() {
                    return Ok(ToolResult::ok("No scheduled jobs."));
                }
                let mut out = format!("Scheduled jobs ({}):\n", jobs.len());
                for job in jobs {
                    let flags = if job.paused && job.one_shot {
                        " [paused, one-shot]"
                    } else if job.paused {
                        " [paused]"
                    } else if job.one_shot {
                        " [one-shot]"
                    } else {
                        ""
                    };
                    out.push_str(&format!(
                        "- {} | {} | status={}{} | cmd: {}\n",
                        job.id,
                        job.expression,
                        job.last_status.as_deref().unwrap_or("pending"),
                        flags,
                        job.command
                    ));
                }
                Ok(ToolResult::ok(out))
            }
            "get" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(job) = scheduler.jobs.get(id) {
                    Ok(ToolResult::ok(format!(
                        "Job {} | {} | cmd: {}",
                        job.id, job.expression, job.command
                    )))
                } else {
                    Ok(ToolResult::fail(format!("Job '{}' not found", id)))
                }
            }
            "create" | "add" => {
                let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                let expression = args
                    .get("expression")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if command.is_empty() || expression.is_empty() {
                    return Ok(ToolResult::fail("Missing 'command' or 'expression'"));
                }
                match scheduler.add_job(
                    expression,
                    command,
                    None,
                    &context.channel,
                    &context.chat_id,
                    &context.session_key,
                ) {
                    Ok(job) => Ok(ToolResult::ok(format!(
                        "Created job {} | {}",
                        job.id, job.expression
                    ))),
                    Err(e) => Ok(ToolResult::fail(format!("Error: {}", e))),
                }
            }
            "once" => {
                let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                let delay = args.get("delay").and_then(|v| v.as_str()).unwrap_or("");
                if command.is_empty() || delay.is_empty() {
                    return Ok(ToolResult::fail("Missing 'command' or 'delay'"));
                }
                match scheduler.add_job(
                    "",
                    command,
                    Some(delay),
                    &context.channel,
                    &context.chat_id,
                    &context.session_key,
                ) {
                    Ok(job) => Ok(ToolResult::ok(format!("Created one-shot task {}", job.id))),
                    Err(e) => Ok(ToolResult::fail(format!("Error: {}", e))),
                }
            }
            "cancel" | "remove" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if scheduler.remove_job(id) {
                    Ok(ToolResult::ok(format!("Cancelled job {}", id)))
                } else {
                    Ok(ToolResult::fail(format!("Job '{}' not found", id)))
                }
            }
            "pause" | "resume" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(job) = scheduler.jobs.get_mut(id) {
                    job.paused = action == "pause";
                    scheduler.save();
                    Ok(ToolResult::ok(format!(
                        "{} job {}",
                        if action == "pause" {
                            "Paused"
                        } else {
                            "Resumed"
                        },
                        id
                    )))
                } else {
                    Ok(ToolResult::fail(format!("Job '{}' not found", id)))
                }
            }
            _ => Ok(ToolResult::fail(format!("Unknown action '{}'", action))),
        }
    }
}
