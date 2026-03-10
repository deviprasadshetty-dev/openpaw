use super::{Tool, ToolContext, ToolResult, path_security};
use anyhow::Result;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct FileWriteTool {
    pub workspace_dir: String,
    pub allowed_paths: Vec<String>,
}

use async_trait::async_trait;

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Write contents to a file in the workspace"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Relative path to the file within the workspace"},"content":{"type":"string","description":"Content to write to the file"}},"required":["path","content"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let path_str = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolResult::fail("Missing 'path' parameter")),
        };

        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolResult::fail("Missing 'content' parameter")),
        };

        let full_path = if Path::new(path_str).is_absolute() {
            if self.allowed_paths.is_empty() {
                return Ok(ToolResult::fail(
                    "Absolute paths not allowed (no allowed_paths configured)",
                ));
            }
            if path_str.contains('\0') {
                return Ok(ToolResult::fail("Path contains null bytes"));
            }
            PathBuf::from(path_str)
        } else {
            if !path_security::is_path_safe(path_str) {
                return Ok(ToolResult::fail(
                    "Path not allowed: contains traversal or absolute path",
                ));
            }
            Path::new(&self.workspace_dir).join(path_str)
        };

        // Resolve parent directory to check permissions (target file might not exist)
        let parent = full_path.parent().unwrap_or(&full_path);
        if let Err(e) = fs::create_dir_all(parent) {
            return Ok(ToolResult::fail(format!(
                "Failed to create directory: {}",
                e
            )));
        }

        // Canonicalize parent to check allowlist
        let resolved_parent = match fs::canonicalize(parent) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult::fail(format!(
                    "Failed to resolve parent path: {}",
                    e
                )));
            }
        };

        let ws_resolved = fs::canonicalize(&self.workspace_dir).unwrap_or_default();

        if !path_security::is_resolved_path_allowed(
            &resolved_parent,
            &ws_resolved,
            &self.allowed_paths,
        ) {
            return Ok(ToolResult::fail("Path is outside allowed areas"));
        }

        // Write to temp file first
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp_name = format!(".openpaw-write-{}.tmp", timestamp);
        let tmp_path = resolved_parent.join(&tmp_name);

        let mut file = match fs::File::create(&tmp_path) {
            Ok(f) => f,
            Err(e) => {
                return Ok(ToolResult::fail(format!(
                    "Failed to create temporary file: {}",
                    e
                )));
            }
        };

        if let Err(e) = file.write_all(content.as_bytes()) {
            let _ = fs::remove_file(&tmp_path);
            return Ok(ToolResult::fail(format!("Failed to write file: {}", e)));
        }

        let target_path = resolved_parent.join(full_path.file_name().unwrap_or_default());
        if let Err(e) = fs::rename(&tmp_path, &target_path) {
            let _ = fs::remove_file(&tmp_path);
            return Ok(ToolResult::fail(format!("Failed to replace file: {}", e)));
        }

        Ok(ToolResult::ok(format!(
            "Written {} bytes to {}",
            content.len(),
            path_str
        )))
    }
}
