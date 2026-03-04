use super::{Tool, ToolResult, path_security};
use anyhow::Result;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub struct FileReadTool {
    pub workspace_dir: String,
    pub allowed_paths: Vec<String>,
    pub max_file_size: u64,
}

impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file in the workspace"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Relative path to the file within the workspace"}},"required":["path"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let path_str = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolResult::fail("Missing 'path' parameter")),
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

        let resolved = match fs::canonicalize(&full_path) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult::fail(format!(
                    "Failed to resolve file path: {}",
                    e
                )));
            }
        };

        let ws_resolved = fs::canonicalize(&self.workspace_dir).unwrap_or_default();

        if !path_security::is_resolved_path_allowed(&resolved, &ws_resolved, &self.allowed_paths) {
            return Ok(ToolResult::fail("Path is outside allowed areas"));
        }

        let metadata = match fs::metadata(&resolved) {
            Ok(m) => m,
            Err(e) => return Ok(ToolResult::fail(format!("Failed to open file: {}", e))),
        };

        if metadata.len() > self.max_file_size {
            return Ok(ToolResult::fail(format!(
                "File too large: {} bytes (limit: {} bytes)",
                metadata.len(),
                self.max_file_size
            )));
        }

        match fs::read_to_string(&resolved) {
            Ok(contents) => Ok(ToolResult::ok(contents)),
            Err(e) => Ok(ToolResult::fail(format!("Failed to read file: {}", e))),
        }
    }

    fn cacheable(&self) -> bool {
        true
    }
}
