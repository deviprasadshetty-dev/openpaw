use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use std::sync::Arc;

pub struct CronAddTool {
    pub cron: Arc<crate::cron::CronScheduler>,
    pub default_timezone: String,
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
        r#"{"type":"object","properties":{"expression":{"type":"string","description":"Cron expression (e.g. '*/5 * * * *' = every 5 minutes, '0 9 * * *' = every day at 9am)"},"delay":{"type":"string","description":"One-shot delay before firing (e.g. '30s', '10m', '2h')"},"command":{"type":"string","description":"Shell command to run, or prompt to send to agent (if job_type is 'agent')"},"name":{"type":"string","description":"Human-readable name for the job"},"job_type":{"type":"string","enum":["shell","agent"],"description":"'shell' runs a shell command; 'agent' sends the command as a prompt to the AI agent (default: shell)"},"deliver_output":{"type":"boolean","description":"Whether to send job output back to this chat (default: true)"},"timezone":{"type":"string","description":"Timezone for cron expression (e.g. 'America/New_York', default: UTC)"}},"required":["command"]}"#.to_string()
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
                "Must provide either 'expression' (cron schedule) or 'delay' (one-shot, e.g. '30m')",
            ));
        }

        let name = args.get("name").and_then(|v| v.as_str());
        let deliver = args
            .get("deliver_output")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        // Get timezone from args or use default
        let timezone = args
            .get("timezone")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.default_timezone)
            .to_string();

        // BUG-CRON-3 FIX: Use bare chat_id, NOT session_key.
        // session_key has format "telegram:1234567" which Telegram API rejects.
        let delivery = if deliver {
            crate::cron::DeliveryConfig {
                mode: crate::cron::DeliveryMode::Always,
                channel: Some(context.channel.clone()),
                account_id: Some(context.sender_id.clone()),
                to: Some(context.chat_id.clone()), // bare chat ID, not session key
                best_effort: true,
            }
        } else {
            crate::cron::DeliveryConfig::default()
        };

        // Parse job_type
        let job_type = match args
            .get("job_type")
            .and_then(|v| v.as_str())
            .unwrap_or("shell")
        {
            "agent" => crate::cron::JobType::Agent,
            _ => crate::cron::JobType::Shell,
        };

        // Generate a unique ID: timestamp + slugified name
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let base_name = name.unwrap_or("job");
        let slug: String = base_name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let job_id = format!("{}-{}", slug, now_secs);

        // Resolve the cron expression
        let expression_str = if let Some(expr) = expression {
            expr.to_string()
        } else if let Some(d) = delay {
            format!("@once:{}", d)
        } else {
            unreachable!()
        };

        let job = crate::cron::CronJob {
            id: job_id.clone(),
            expression: expression_str.clone(),
            command: command.to_string(),
            job_type,
            session_target: crate::cron::SessionTarget::Isolated,
            delivery,
            enabled: true,
            created_at_s: now_secs as i64,
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
            timezone,
        };

        self.cron.add_job(job);
        self.cron.save(); // Persist immediately so it survives restarts

        let schedule_desc = if delay.is_some() {
            format!("fires once after `{}`", delay.unwrap_or(""))
        } else {
            format!("schedule: `{}`", expression_str)
        };

        Ok(ToolResult::ok(format!(
            "Created cron job `{}` ({}) → `{}`",
            job_id, schedule_desc, command
        )))
    }
}
