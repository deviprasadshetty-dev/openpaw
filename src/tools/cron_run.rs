use super::{Tool, ToolContext, ToolResult};
use crate::tools::cron_utils::CronScheduler;
use anyhow::Result;
use serde_json::Value;
use std::process::Command;

pub struct CronRunTool {}

impl Tool for CronRunTool {
    fn name(&self) -> &str {
        "cron_run"
    }
    fn description(&self) -> &str {
        "Force-run a cron job immediately by its ID, regardless of schedule."
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"job_id":{"type":"string","description":"The ID of the cron job to run"}},"required":["job_id"]}"#.to_string()
    }

    fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let job_id = match args.get("job_id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id,
            _ => return Ok(ToolResult::fail("Missing 'job_id' parameter")),
        };

        let mut scheduler = CronScheduler::new();
        let job = match scheduler.jobs.get(job_id) {
            Some(j) => j.clone(),
            None => return Ok(ToolResult::fail(format!("Job '{}' not found", job_id))),
        };

        let sh = if cfg!(target_os = "windows") {
            "cmd"
        } else {
            "sh"
        };
        let flag = if cfg!(target_os = "windows") {
            "/C"
        } else {
            "-c"
        };

        let (exit_code, output_str) = match Command::new(sh).args(&[flag, &job.command]).output() {
            Ok(out) => {
                let code = out.status.code().unwrap_or(1);
                let text = String::from_utf8_lossy(&if out.stdout.is_empty() {
                    out.stderr
                } else {
                    out.stdout
                })
                .to_string();
                (code, text)
            }
            Err(e) => (1, format!("Error: {}", e)),
        };

        if let Some(j) = scheduler.jobs.get_mut(job_id) {
            let status = if exit_code == 0 { "success" } else { "error" };
            j.last_status = Some(status.to_string());
            use std::time::{SystemTime, UNIX_EPOCH};
            j.last_run_secs = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            );
            scheduler.save();
        }

        let status_label = if exit_code == 0 { "ok" } else { "error" };
        Ok(ToolResult::ok(format!(
            "Job {} ran: {} (exit {})\n{}",
            job_id, status_label, exit_code, output_str
        )))
    }
}
