use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::fs;
use std::path::Path;

pub struct FileDeleteTool {
    pub workspace_dir: String,
    pub allowed_paths: Vec<String>,
}

#[async_trait]
impl Tool for FileDeleteTool {
    fn name(&self) -> &str {
        "file_delete"
    }

    fn description(&self) -> &str {
        "Delete BOOTSTRAP.md or other temporary workspace files when they are no longer needed."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file to delete (e.g. 'BOOTSTRAP.md')"
                }
            },
            "required": ["path"]
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let rel_path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolResult::fail("Missing required parameter: path")),
        };

        if rel_path.contains("..") || Path::new(rel_path).is_absolute() {
            return Ok(ToolResult::fail(
                "Invalid path: must be workspace-relative and not contain '..'",
            ));
        }

        // Only allow deleting BOOTSTRAP.md for now to match NullClaw's safety
        if rel_path != "BOOTSTRAP.md" && !rel_path.ends_with(".tmp") {
            return Ok(ToolResult::fail(
                "Only BOOTSTRAP.md or temporary .tmp files can be deleted with this tool for safety.",
            ));
        }

        let full_path = Path::new(&self.workspace_dir).join(rel_path);

        // Canonicalize to verify it's inside allowed paths
        let canonical_path = match fs::canonicalize(&full_path) {
            Ok(p) => p,
            Err(_) => return Ok(ToolResult::fail("File not found or inaccessible")),
        };

        let mut allowed = false;
        for allowed_prefix in &self.allowed_paths {
            if let Ok(ap) = fs::canonicalize(allowed_prefix) {
                if canonical_path.starts_with(ap) {
                    allowed = true;
                    break;
                }
            }
        }

        if !allowed {
            return Ok(ToolResult::fail(
                "Access denied: path is outside of allowed workspace areas",
            ));
        }

        match fs::remove_file(&canonical_path) {
            Ok(_) => Ok(ToolResult::ok(format!("Successfully deleted {}", rel_path))),
            Err(e) => Ok(ToolResult::fail(format!("Failed to delete file: {}", e))),
        }
    }
}
