use super::{Tool, ToolContext, ToolResult, path_security};
use anyhow::Result;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct FileAppendTool {
    pub workspace_dir: String,
    pub allowed_paths: Vec<String>,
    pub max_file_size: usize,
}

use async_trait::async_trait;

#[async_trait]
impl Tool for FileAppendTool {
    fn name(&self) -> &str {
        "file_append"
    }

    fn description(&self) -> &str {
        "Append content to the end of a file (creates the file if it doesn't exist)"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Relative path to the file within the workspace"},"content":{"type":"string","description":"Content to append to the file"}},"required":["path","content"]}"#.to_string()
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

        let ws_resolved = fs::canonicalize(&self.workspace_dir).unwrap_or_default();

        let existing_content = if full_path.exists() {
            let resolved = match fs::canonicalize(&full_path) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(ToolResult::fail(format!(
                        "Failed to resolve file path: {}",
                        e
                    )));
                }
            };

            if !path_security::is_resolved_path_allowed(
                &resolved,
                &ws_resolved,
                &self.allowed_paths,
            ) {
                return Ok(ToolResult::fail("Path is outside allowed areas"));
            }

            let metadata = match fs::metadata(&resolved) {
                Ok(m) => m,
                Err(e) => return Ok(ToolResult::fail(format!("Failed to open file: {}", e))),
            };

            if metadata.len() > self.max_file_size as u64 {
                return Ok(ToolResult::fail(format!(
                    "File too large: {} bytes (limit: {} bytes)",
                    metadata.len(),
                    self.max_file_size
                )));
            }

            match fs::read_to_string(&resolved) {
                Ok(c) => Some(c),
                Err(e) => return Ok(ToolResult::fail(format!("Failed to read file: {}", e))),
            }
        } else {
            // If file doesn't exist, check parent permissions/allowlist for creation
            let parent = full_path.parent().unwrap_or(&full_path);
            if let Err(e) = fs::create_dir_all(parent) {
                return Ok(ToolResult::fail(format!(
                    "Failed to create directory: {}",
                    e
                )));
            }
            let resolved_parent = match fs::canonicalize(parent) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(ToolResult::fail(format!(
                        "Failed to resolve parent path: {}",
                        e
                    )));
                }
            };
            if !path_security::is_resolved_path_allowed(
                &resolved_parent,
                &ws_resolved,
                &self.allowed_paths,
            ) {
                return Ok(ToolResult::fail("Path is outside allowed areas"));
            }
            None
        };

        let new_content = if let Some(existing) = existing_content {
            format!("{}{}", existing, content)
        } else {
            content.to_string()
        };

        // Write via temp file + rename
        let parent = full_path.parent().unwrap_or(&full_path);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp_name = format!(".openpaw-write-{}.tmp", timestamp);
        let tmp_path = parent.join(&tmp_name);

        let mut file = match fs::File::create(&tmp_path) {
            Ok(f) => f,
            Err(e) => {
                return Ok(ToolResult::fail(format!(
                    "Failed to create temporary file: {}",
                    e
                )));
            }
        };

        if let Err(e) = file.write_all(new_content.as_bytes()) {
            let _ = fs::remove_file(&tmp_path);
            return Ok(ToolResult::fail(format!("Failed to write file: {}", e)));
        }

        let target_path = if full_path.is_absolute() {
            full_path
        } else {
            // Need absolute path for rename if full_path was relative but we have parent which might be relative?
            // Actually parent.join(filename) is fine.
            full_path
        };

        if let Err(e) = fs::rename(&tmp_path, &target_path) {
            let _ = fs::remove_file(&tmp_path);
            return Ok(ToolResult::fail(format!("Failed to replace file: {}", e)));
        }

        Ok(ToolResult::ok(format!(
            "Appended {} bytes to {}",
            content.len(),
            path_str
        )))
    }
}
