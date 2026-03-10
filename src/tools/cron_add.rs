use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use std::sync::Arc;

pub struct CronAddTool {
    pub cron: Arc<crate::cron::CronScheduler>,
}

#[async_trait]
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

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolResult> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolResult::fail("Missing required 'command' parameter")),
        };

        let expression = args.get("expression").and_then(|v| v.as_str());
        let delay = args.get("delay").and_then(|v| v.as_str());

        if expression.is_none() && delay.is_none() {
            return Ok(ToolResult::fail(
                "Must provide either 'expression' or 'delay'",
            ));
        }

        let name = args.get("name").and_then(|v| v.as_str());

        let job = crate::cron::CronJob {
            id: name.unwrap_or("unnamed").to_string(),
            expression: expression.unwrap_or("").to_string(),
            command: command.to_string(),
            job_type: crate::cron::JobType::Shell,
            session_target: crate::cron::SessionTarget::Isolated,
            delivery: crate::cron::DeliveryConfig {
                mode: crate::cron::DeliveryMode::Always,
                channel: Some(context.channel.clone()),
                account_id: Some(context.sender_id.clone()),
                to: Some(context.session_key.clone()),
                best_effort: true,
            },
            enabled: true,
            created_at_s: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            next_run_secs: 0,
            last_run_secs: None,
            last_status: None,
            paused: false,
            one_shot: delay.is_some(),
            prompt: None,
            name: name.map(|s| s.to_string()),
            model: None,
            delete_after_run: delay.is_some(),
            last_output: None,
            history: Vec::new(),
        };

        self.cron.add_job(job.clone());

        Ok(ToolResult::ok(format!(
            "Created cron job {}: {} -> {}",
            job.id, job.expression, job.command
        )))
    }
}
