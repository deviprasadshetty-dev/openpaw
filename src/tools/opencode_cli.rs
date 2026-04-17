use super::{Tool, ToolContext, ToolResult, path_security, process_util};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct OpencodeCliTool {
    pub workspace_dir: String,
    pub allowed_paths: Vec<String>,
    pub binary: String,
    pub default_attach_url: Option<String>,
    pub timeout_secs: u64,
    pub max_output_bytes: usize,
}

#[async_trait]
impl Tool for OpencodeCliTool {
    fn name(&self) -> &str {
        "opencode_cli"
    }

    fn description(&self) -> &str {
        "Run OpenCode CLI prompts (opencode run) for deep reasoning tasks: coding, research synthesis, planning, writing, and second-opinion analysis"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"prompt":{"type":"string","description":"Prompt to send to OpenCode"},"model":{"type":"string","description":"Model in provider/model format"},"agent":{"type":"string","description":"OpenCode agent name"},"attach_url":{"type":"string","description":"Attach to a running opencode serve/web endpoint (for example http://localhost:4096)"},"continue":{"type":"boolean","description":"Continue the last OpenCode session"},"session":{"type":"string","description":"Session ID to continue"},"fork":{"type":"boolean","description":"Fork the session when continuing"},"files":{"type":"array","items":{"type":"string"},"description":"File paths to attach to the prompt"},"title":{"type":"string","description":"Optional session title"},"format":{"type":"string","enum":["default","json"],"description":"Output format from opencode run"},"cwd":{"type":"string","description":"Working directory (absolute path within allowed paths; defaults to workspace)"}},"required":["prompt"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let prompt = match args.get("prompt").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p,
            _ => return Ok(ToolResult::fail("Missing 'prompt' parameter")),
        };

        let effective_cwd = match self.resolve_cwd(args.get("cwd").and_then(|v| v.as_str())) {
            Ok(path) => path,
            Err(msg) => return Ok(ToolResult::fail(msg)),
        };

        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        if format != "default" && format != "json" {
            return Ok(ToolResult::fail(
                "Invalid format. Allowed values are: default, json",
            ));
        }

        let model = args.get("model").and_then(|v| v.as_str());
        let agent = args.get("agent").and_then(|v| v.as_str());
        let session = args.get("session").and_then(|v| v.as_str());
        let title = args.get("title").and_then(|v| v.as_str());
        let attach_url = args
            .get("attach_url")
            .and_then(|v| v.as_str())
            .or(self.default_attach_url.as_deref());
        let continue_last = args
            .get("continue")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let fork = args.get("fork").and_then(|v| v.as_bool()).unwrap_or(false);

        let attached_files = match self.resolve_attached_files(&effective_cwd, args.get("files")) {
            Ok(files) => files,
            Err(msg) => return Ok(ToolResult::fail(msg)),
        };

        let mut argv_owned: Vec<String> = vec![self.binary.clone(), "run".to_string()];

        if continue_last {
            argv_owned.push("--continue".to_string());
        }
        if let Some(s) = session.filter(|s| !s.trim().is_empty()) {
            argv_owned.push("--session".to_string());
            argv_owned.push(s.to_string());
        }
        if fork {
            argv_owned.push("--fork".to_string());
        }
        if let Some(m) = model.filter(|m| !m.trim().is_empty()) {
            argv_owned.push("--model".to_string());
            argv_owned.push(m.to_string());
        }
        if let Some(a) = agent.filter(|a| !a.trim().is_empty()) {
            argv_owned.push("--agent".to_string());
            argv_owned.push(a.to_string());
        }
        if let Some(t) = title.filter(|t| !t.trim().is_empty()) {
            argv_owned.push("--title".to_string());
            argv_owned.push(t.to_string());
        }
        if let Some(url) = attach_url.filter(|u| !u.trim().is_empty()) {
            argv_owned.push("--attach".to_string());
            argv_owned.push(url.to_string());
        }
        if format == "json" {
            argv_owned.push("--format".to_string());
            argv_owned.push("json".to_string());
        }
        for file in attached_files {
            argv_owned.push("--file".to_string());
            argv_owned.push(file);
        }

        argv_owned.push(prompt.to_string());

        let argv: Vec<&str> = argv_owned.iter().map(String::as_str).collect();
        let opts = process_util::RunOptions {
            cwd: Some(Path::new(&effective_cwd)),
            timeout_ms: self.timeout_secs.saturating_mul(1000),
            max_output_bytes: self.max_output_bytes,
            ..Default::default()
        };

        let result = process_util::run(&argv, opts).await?;

        if result.timed_out {
            return Ok(ToolResult::fail("OpenCode CLI command timed out"));
        }

        if result.success {
            let mut output = String::new();
            if !result.stdout.trim().is_empty() {
                output.push_str(&result.stdout);
            }
            if !result.stderr.trim().is_empty() {
                if !output.is_empty() {
                    output.push_str("\n\n[stderr]\n");
                }
                output.push_str(&result.stderr);
            }
            if output.trim().is_empty() {
                output = "(no output)".to_string();
            }
            Ok(ToolResult::ok(output))
        } else {
            let msg = if !result.stderr.trim().is_empty() {
                result.stderr
            } else if !result.stdout.trim().is_empty() {
                result.stdout
            } else if let Some(code) = result.exit_code {
                format!("OpenCode CLI failed with exit code {}", code)
            } else {
                "OpenCode CLI failed".to_string()
            };
            Ok(ToolResult::fail(msg))
        }
    }
}

impl OpencodeCliTool {
    fn resolve_cwd(&self, cwd: Option<&str>) -> std::result::Result<String, String> {
        if let Some(raw_cwd) = cwd {
            let cwd_path = Path::new(raw_cwd);
            if !cwd_path.is_absolute() {
                return Err("cwd must be an absolute path".to_string());
            }
            let resolved_cwd = std::fs::canonicalize(cwd_path)
                .map_err(|e| format!("Failed to resolve cwd: {}", e))?;
            let ws_resolved = std::fs::canonicalize(&self.workspace_dir).unwrap_or_default();
            if !path_security::is_resolved_path_allowed(
                &resolved_cwd,
                &ws_resolved,
                &self.allowed_paths,
            ) {
                return Err("cwd is outside allowed areas".to_string());
            }
            Ok(resolved_cwd.to_string_lossy().to_string())
        } else {
            Ok(self.workspace_dir.clone())
        }
    }

    fn resolve_attached_files(
        &self,
        effective_cwd: &str,
        files_value: Option<&Value>,
    ) -> std::result::Result<Vec<String>, String> {
        let Some(files) = files_value else {
            return Ok(Vec::new());
        };

        let arr = files
            .as_array()
            .ok_or_else(|| "'files' must be an array of paths".to_string())?;

        let mut resolved = Vec::new();
        let workspace_resolved = std::fs::canonicalize(&self.workspace_dir).unwrap_or_default();

        for item in arr {
            let raw = item
                .as_str()
                .ok_or_else(|| "Each item in 'files' must be a string path".to_string())?;

            if !path_security::is_path_safe(raw) {
                return Err(format!("Unsafe file path: {}", raw));
            }

            let candidate = if Path::new(raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                Path::new(effective_cwd).join(raw)
            };

            let canonical = std::fs::canonicalize(&candidate)
                .map_err(|e| format!("Failed to resolve file path '{}': {}", raw, e))?;

            if !path_security::is_resolved_path_allowed(
                &canonical,
                &workspace_resolved,
                &self.allowed_paths,
            ) {
                return Err(format!("File path is outside allowed areas: {}", raw));
            }

            resolved.push(canonical.to_string_lossy().to_string());
        }

        Ok(resolved)
    }
}
