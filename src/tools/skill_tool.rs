use super::{Tool, ToolContext, ToolResult};
use crate::skills::SkillToolDefinition;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use tokio::process::Command;

pub struct DynamicSkillTool {
    pub definition: SkillToolDefinition,
    pub skill_path: PathBuf,
}

#[async_trait]
impl Tool for DynamicSkillTool {
    fn name(&self) -> &str {
        &self.definition.name
    }

    fn description(&self) -> &str {
        &self.definition.description
    }

    fn parameters_json(&self) -> String {
        serde_json::to_string(&self.definition.parameters).unwrap_or_else(|_| "{}".to_string())
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let mut cmd_str = self.definition.command.clone();

        if let Some(obj) = args.as_object() {
            for (key, val) in obj {
                let placeholder = format!("{{{{{}}}}}", key);
                let val_str = match val {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => val.to_string(),
                };
                cmd_str = cmd_str.replace(&placeholder, &val_str);
            }
        }

        let output = if cfg!(windows) {
            Command::new("cmd")
                .args(["/C", &cmd_str])
                .current_dir(&self.skill_path)
                .output()
                .await
        } else {
            Command::new("sh")
                .args(["-c", &cmd_str])
                .current_dir(&self.skill_path)
                .output()
                .await
        }
        .context("Failed to execute skill tool command")?;

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        if output.status.success() {
            Ok(ToolResult::ok(stdout))
        } else {
            Ok(ToolResult::fail(format!(
                "Command failed with exit code {}.\nSTDOUT: {}\nSTDERR: {}",
                output.status.code().unwrap_or(-1),
                stdout,
                stderr
            )))
        }
    }
}
