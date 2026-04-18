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
        // ── 0.2: Fix parameter injection ──────────────────────────────────────
        //
        // BROKEN (previous): substitute {{param}} into the entire command string,
        // then pass that string to `sh -c cmd_str`.  This lets any parameter value
        // containing shell metacharacters (`;`, `|`, `$(...)`, etc.) execute
        // arbitrary commands.
        //
        // FIXED: split the command template into individual argv tokens FIRST, then
        // substitute placeholder values into individual tokens.  Each token becomes
        // a separate Command::arg() call — the shell never sees the values.
        //
        // This preserves skill functionality: all existing {{param}} templates keep
        // working, they just can't inject shell metacharacters anymore.

        let cmd_str = self.definition.command.trim();

        // Build the argv template by splitting on whitespace
        let template_argv: Vec<String> = cmd_str.split_whitespace().map(|s| s.to_string()).collect();

        if template_argv.is_empty() {
            return Ok(ToolResult::fail("Skill command is empty"));
        }

        // Substitute {{param}} placeholders in each argv token individually
        let substituted: Vec<String> = template_argv
            .iter()
            .map(|token| {
                let mut t = token.clone();
                if let Some(obj) = args.as_object() {
                    for (key, val) in obj {
                        let placeholder = format!("{{{{{}}}}}", key);
                        let val_str = match val {
                            Value::String(s) => s.clone(),
                            Value::Number(n) => n.to_string(),
                            Value::Bool(b) => b.to_string(),
                            _ => val.to_string(),
                        };
                        t = t.replace(&placeholder, &val_str);
                    }
                }
                t
            })
            .collect();

        // Now build the Command from the substituted argv (no shell involved)
        let mut cmd = build_command_from_argv(&substituted, &self.skill_path);

        let output = cmd
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

/// Build a `Command` from an already-split argv slice, with no shell involvement.
///
/// Routing logic (same as before, but operating on per-token argv):
/// 1. `runtime:<interpreter>` prefix in first token → use that interpreter
/// 2. File extension in first token (e.g. `.py`, `.js`) → use detected interpreter
/// 3. Otherwise → execute the first token as the binary directly (no shell)
fn build_command_from_argv(argv: &[String], cwd: &PathBuf) -> Command {
    if argv.is_empty() {
        let mut c = Command::new("true");
        c.current_dir(cwd);
        return c;
    }

    let first = &argv[0];

    // 1. Explicit runtime prefix: first token is `runtime:<interpreter>`
    if let Some(interpreter) = first.strip_prefix("runtime:") {
        let mut c = Command::new(interpreter);
        // Remaining tokens (index 1+) are arguments to the interpreter
        if argv.len() > 1 {
            c.args(&argv[1..]);
        }
        c.current_dir(cwd);
        return c;
    }

    // 2. Detect interpreter from filename extension
    if let Some(interpreter) = interpreter_for(first) {
        let mut c = Command::new(interpreter);
        // Pass entire argv (script name + args) to interpreter
        c.args(argv);
        c.current_dir(cwd);
        return c;
    }

    // 3. Execute the binary directly — no shell, no `-c`, no injection surface
    let mut c = Command::new(first);
    if argv.len() > 1 {
        c.args(&argv[1..]);
    }
    c.current_dir(cwd);
    c
}

/// Map a file extension to the preferred interpreter binary.
fn interpreter_for(filename: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())?
        .to_lowercase();

    match ext.as_str() {
        "py"   => Some("python3"),
        "js"   => Some("node"),
        "rb"   => Some("ruby"),
        "pl"   => Some("perl"),
        "php"  => Some("php"),
        "r"    => Some("Rscript"),
        "lua"  => Some("lua"),
        "sh"   => Some("sh"),
        "bash" => Some("bash"),
        "zsh"  => Some("zsh"),
        "fish" => Some("fish"),
        "ps1"  => Some("powershell"),
        _      => None,
    }
}
