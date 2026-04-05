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

        // Substitute {{param}} placeholders
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

        let output = build_command(&cmd_str, &self.skill_path)
            .output()
            .await
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

/// Determine how to run `cmd_str` based on:
/// 1. An explicit `runtime:` prefix (e.g. `runtime:python3 script.py`)
/// 2. The file extension of the first token
/// 3. The current platform (Windows vs Unix shell)
fn build_command(cmd_str: &str, cwd: &PathBuf) -> Command {
    // Check for explicit runtime prefix: "runtime:<interpreter> ..."
    if let Some(rest) = cmd_str.strip_prefix("runtime:") {
        let mut parts = rest.splitn(2, ' ');
        let interpreter = parts.next().unwrap_or("sh");
        let remainder = parts.next().unwrap_or("");
        let mut c = Command::new(interpreter);
        if !remainder.is_empty() {
            c.args(remainder.split_whitespace());
        }
        c.current_dir(cwd);
        return c;
    }

    // Detect interpreter from the first token's extension
    let first_token = cmd_str.split_whitespace().next().unwrap_or("");
    if let Some(interpreter) = interpreter_for(first_token) {
        let mut c = Command::new(interpreter);
        // Pass the whole command string as arguments to the interpreter
        c.args(cmd_str.split_whitespace());
        c.current_dir(cwd);
        return c;
    }

    // Default: platform shell
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", cmd_str]);
        c.current_dir(cwd);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", cmd_str]);
        c.current_dir(cwd);
        c
    }
}

/// Map a file extension to the preferred interpreter binary.
fn interpreter_for(filename: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())?
        .to_lowercase();

    match ext.as_str() {
        "py"    => Some("python3"),
        "js"    => Some("node"),
        "ts"    => Some("npx ts-node"),  // common enough to support
        "rb"    => Some("ruby"),
        "pl"    => Some("perl"),
        "php"   => Some("php"),
        "r"     => Some("Rscript"),
        "lua"   => Some("lua"),
        "sh"    => Some("sh"),
        "bash"  => Some("bash"),
        "zsh"   => Some("zsh"),
        "fish"  => Some("fish"),
        "ps1"   => Some("powershell"),
        _       => None,
    }
}
