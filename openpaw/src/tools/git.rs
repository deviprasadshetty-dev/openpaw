use super::{path_security, process_util, Tool, ToolResult};
use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct GitTool {
    pub workspace_dir: String,
    pub allowed_paths: Vec<String>,
}

impl Tool for GitTool {
    fn name(&self) -> &str {
        "git_operations"
    }

    fn description(&self) -> &str {
        "Perform structured Git operations (status, diff, log, branch, commit, add, checkout, stash)."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"operation":{"type":"string","enum":["status","diff","log","branch","commit","add","checkout","stash"],"description":"Git operation to perform"},"message":{"type":"string","description":"Commit message"},"paths":{"type":"string","description":"File paths"},"branch":{"type":"string","description":"Branch name"},"files":{"type":"string","description":"Files to diff"},"cached":{"type":"boolean","description":"Show staged changes"},"limit":{"type":"integer","description":"Log entry count"},"cwd":{"type":"string","description":"Repository directory"}},"required":["operation"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let operation = match args.get("operation").and_then(|v| v.as_str()) {
            Some(op) => op,
            None => return Ok(ToolResult::fail("Missing 'operation' parameter")),
        };

        // Sanitize string args
        for field in &["message", "paths", "branch", "files", "action"] {
             if let Some(val) = args.get(field).and_then(|v| v.as_str()) {
                 if !Self::sanitize_git_args(val) {
                     return Ok(ToolResult::fail("Unsafe git arguments detected"));
                 }
             }
        }

        // Resolve cwd
        let effective_cwd = if let Some(cwd) = args.get("cwd").and_then(|v| v.as_str()) {
             let cwd_path = Path::new(cwd);
             if !cwd_path.is_absolute() {
                 return Ok(ToolResult::fail("cwd must be an absolute path"));
             }
             let resolved_cwd = match std::fs::canonicalize(cwd_path) {
                 Ok(p) => p,
                 Err(e) => return Ok(ToolResult::fail(format!("Failed to resolve cwd: {}", e))),
             };
             let ws_resolved = std::fs::canonicalize(&self.workspace_dir).unwrap_or_default();
             
             if !path_security::is_resolved_path_allowed(&resolved_cwd, &ws_resolved, &self.allowed_paths) {
                 return Ok(ToolResult::fail("cwd is outside allowed areas"));
             }
             resolved_cwd.to_string_lossy().to_string()
        } else {
             self.workspace_dir.clone()
        };

        match operation {
            "status" => self.run_git(&effective_cwd, &["status", "--porcelain=2", "--branch"]),
            "diff" => {
                let cached = args.get("cached").and_then(|v| v.as_bool()).unwrap_or(false);
                let files = args.get("files").and_then(|v| v.as_str()).unwrap_or(".");
                let mut args = vec!["diff", "--unified=3"];
                if cached { args.push("--cached"); }
                args.push("--");
                args.push(files);
                self.run_git(&effective_cwd, &args)
            },
            "log" => {
                let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(10).clamp(1, 1000);
                let limit_arg = format!("-{}", limit);
                self.run_git(&effective_cwd, &["log", &limit_arg, "--pretty=format:%H|%an|%ae|%ad|%s", "--date=iso"])
            },
            "branch" => self.run_git(&effective_cwd, &["branch", "--format=%(refname:short)|%(HEAD)"]),
            "commit" => {
                 let msg = args.get("message").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing message"))?;
                 if msg.is_empty() { return Ok(ToolResult::fail("Commit message cannot be empty")); }
                 self.run_git(&effective_cwd, &["commit", "-m", msg])
            },
            "add" => {
                 let paths = args.get("paths").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing paths"))?;
                 self.run_git(&effective_cwd, &["add", "--", paths])
            },
            "checkout" => {
                 let branch = args.get("branch").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing branch"))?;
                 self.run_git(&effective_cwd, &["checkout", branch])
            },
            "stash" => {
                 // stash push/pop/list
                 self.run_git(&effective_cwd, &["stash", "list"]) // Simplified for now
            },
            _ => Ok(ToolResult::fail(format!("Unknown operation: {}", operation))),
        }
    }
}

impl GitTool {
    fn sanitize_git_args(arg: &str) -> bool {
        let dangerous_prefixes = &[
            "--exec=", "--upload-pack=", "--receive-pack=", "--pager=", "--editor=",
        ];
        let dangerous_exact = &["--no-verify"];
        let dangerous_substrings = &["$(", "`"];
        let dangerous_chars = &['|', ';', '>'];

        for part in arg.split_whitespace() {
             for prefix in dangerous_prefixes {
                 if part.to_lowercase().starts_with(prefix) { return false; }
             }
             for exact in dangerous_exact {
                 if part.eq_ignore_ascii_case(exact) { return false; }
             }
             for sub in dangerous_substrings {
                 if part.contains(sub) { return false; }
             }
             if part.contains(dangerous_chars) { return false; }
             
             // -c config injection
             if part == "-c" || part == "-C" { return false; }
             if part.starts_with("-c=") || part.starts_with("-C=") { return false; }
        }
        true
    }

    fn run_git(&self, cwd: &str, args: &[&str]) -> Result<ToolResult> {
        let mut cmd_args = vec!["git"];
        cmd_args.extend_from_slice(args);
        
        // Remove "git" from args passed to process_util::run because it expects the command as first arg
        // Wait, process_util::run expects full argv including command?
        // My implementation: `let mut cmd = Command::new(args[0]);`
        // So yes.
        
        let opts = process_util::RunOptions {
            cwd: Some(Path::new(cwd)),
            ..Default::default()
        };

        let result = process_util::run(&cmd_args, opts)?;

        if result.success {
            Ok(ToolResult::ok(result.stdout))
        } else {
            let msg = if !result.stderr.is_empty() { result.stderr } else { "Git operation failed".to_string() };
            Ok(ToolResult::fail(msg))
        }
    }
}
