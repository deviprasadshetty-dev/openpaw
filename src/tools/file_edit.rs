use super::{Tool, ToolContext, ToolResult, path_security};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct FileEditTool {
    pub workspace_dir: String,
    pub allowed_paths: Vec<String>,
    pub max_file_size: usize,
}

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "file_edit"
    }

    fn description(&self) -> &str {
        "Edit a file using search and replace"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Relative path to the file"},"search":{"type":"string","description":"String to find"},"replace":{"type":"string","description":"String to replace with"}},"required":["path","search","replace"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let path_str = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolResult::fail("Missing 'path' parameter")),
        };
        let search = match args.get("search").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return Ok(ToolResult::fail("Missing 'search' parameter")),
        };
        let replace = match args.get("replace").and_then(|v| v.as_str()) {
            Some(r) => r,
            None => return Ok(ToolResult::fail("Missing 'replace' parameter")),
        };

        if search.is_empty() {
            return Ok(ToolResult::fail("search must not be empty"));
        }

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

        if metadata.len() > self.max_file_size as u64 {
            return Ok(ToolResult::fail(format!(
                "File too large: {} bytes (limit: {} bytes)",
                metadata.len(),
                self.max_file_size
            )));
        }

        let content = match fs::read_to_string(&resolved) {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::fail(format!("Failed to read file: {}", e))),
        };

        if !content.contains(search) {
            return Ok(ToolResult::fail("search string not found in file"));
        }

        let new_content = content.replacen(search, replace, 1);

        // Write back directly
        let mut file = match fs::File::create(&resolved) {
            Ok(f) => f,
            Err(e) => {
                return Ok(ToolResult::fail(format!(
                    "Failed to open file for writing: {}",
                    e
                )));
            }
        };

        if let Err(e) = file.write_all(new_content.as_bytes()) {
            return Ok(ToolResult::fail(format!("Failed to write file: {}", e)));
        }

        Ok(ToolResult::ok(format!(
            "Replaced '{}' with '{}' in {}",
            search, replace, path_str
        )))
    }
}
