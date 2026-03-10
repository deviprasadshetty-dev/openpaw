use super::{Tool, ToolContext, ToolResult, path_security, process_util};
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

const SAFE_ENV_VARS: &[&str] = &[
    "PATH", "HOME", "TERM", "LANG", "LC_ALL", "LC_CTYPE", "USER", "SHELL", "TMPDIR",
];

pub struct ShellTool {
    pub workspace_dir: String,
    pub allowed_paths: Vec<String>,
    pub timeout_ns: u64,
    pub max_output_bytes: usize,
}

use async_trait::async_trait;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command in the workspace directory"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"command":{"type":"string","description":"The shell command to execute"},"cwd":{"type":"string","description":"Working directory (absolute path within allowed paths; defaults to workspace)"}},"required":["command"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolResult::fail("Missing 'command' parameter")),
        };

        // Determine working directory
        let effective_cwd = if let Some(cwd) = args.get("cwd").and_then(|v| v.as_str()) {
            let cwd_path = Path::new(cwd);
            if !cwd_path.is_absolute() {
                return Ok(ToolResult::fail("cwd must be an absolute path"));
            }

            // Resolve canonical path
            let resolved_cwd = match std::fs::canonicalize(cwd_path) {
                Ok(p) => p,
                Err(e) => return Ok(ToolResult::fail(format!("Failed to resolve cwd: {}", e))),
            };

            // Resolve workspace path
            let ws_resolved = std::fs::canonicalize(&self.workspace_dir).unwrap_or_default();

            if !path_security::is_resolved_path_allowed(
                &resolved_cwd,
                &ws_resolved,
                &self.allowed_paths,
            ) {
                return Ok(ToolResult::fail("cwd is outside allowed areas"));
            }
            resolved_cwd.to_string_lossy().to_string()
        } else {
            self.workspace_dir.clone()
        };

        // Filter environment
        let mut env_map = HashMap::new();
        for &key in SAFE_ENV_VARS {
            if let Ok(val) = std::env::var(key) {
                env_map.insert(key.to_string(), val);
            }
        }

        // Prepare run arguments
        #[cfg(unix)]
        let argv = vec!["/bin/sh", "-c", command];
        #[cfg(windows)]
        let argv = vec!["cmd.exe", "/c", command];

        let opts = process_util::RunOptions {
            cwd: Some(Path::new(&effective_cwd)),
            env_clear: true,
            env_vars: Some(&env_map),
            max_output_bytes: self.max_output_bytes,
            timeout_ms: self.timeout_ns / 1_000_000, // Convert ns to ms
        };

        let result = process_util::run(&argv, opts).await?;

        if result.timed_out {
            return Ok(ToolResult::fail("Command timed out"));
        }

        if result.success {
            let output = if !result.stdout.is_empty() {
                result.stdout
            } else {
                "(no output)".to_string()
            };
            Ok(ToolResult::ok(output))
        } else {
            let err_msg = if !result.stderr.is_empty() {
                result.stderr
            } else if let Some(code) = result.exit_code {
                format!("Command failed with non-zero exit code: {}", code)
            } else {
                "Command terminated by signal".to_string()
            };
            Ok(ToolResult::fail(err_msg))
        }
    }
}
