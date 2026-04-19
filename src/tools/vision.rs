use crate::tools::{Tool, ToolContext, ToolResult, path_security};
use crate::multimodal::{is_gemini_cli_available, process_with_gemini_cli};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct VisionTool {
    pub workspace_dir: String,
    pub allowed_paths: Vec<String>,
}

#[async_trait]
impl Tool for VisionTool {
    fn name(&self) -> &str {
        "vision"
    }

    fn description(&self) -> &str {
        "Analyze images, videos, audio, and documents with Gemini CLI when available. Use this for local file/media understanding; use web_search for internet results."
    }

    fn parameters_json(&self) -> String {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "What you want to know about the file(s). Be specific."
                },
                "files": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "description": "List of file paths to analyze."
                }
            },
            "required": ["prompt", "files"]
        }).to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let prompt = args["prompt"].as_str().unwrap_or("What is in this file?");
        let files_val = &args["files"];
        
        let mut files = Vec::new();
        if let Some(arr) = files_val.as_array() {
            for v in arr {
                if let Some(s) = v.as_str() {
                    files.push(s.to_string());
                }
            }
        } else if let Some(s) = files_val.as_str() {
            files.push(s.to_string());
        }

        if files.is_empty() {
            return Ok(ToolResult::fail(json!({ "error": "No files provided" }).to_string()));
        }

        let validated_files = match self.validate_and_resolve_files(&files) {
            Ok(v) => v,
            Err(msg) => return Ok(ToolResult::fail(msg)),
        };

        if is_gemini_cli_available() {
            match process_with_gemini_cli(prompt, &validated_files) {
                Ok(res) => Ok(ToolResult::ok(format!(
                    "Vision analysis (gemini-cli):\n{}",
                    res
                ))),
                Err(e) => Ok(ToolResult::fail(format!("Gemini CLI failed: {}", e))),
            }
        } else {
            Ok(ToolResult::fail(
                "Gemini CLI not available on this system. Install with: npm install -g @google/gemini-cli"
                    .to_string(),
            ))
        }
    }
}

impl VisionTool {
    fn validate_and_resolve_files(&self, files: &[String]) -> std::result::Result<Vec<String>, String> {
        let workspace_resolved = std::fs::canonicalize(&self.workspace_dir)
            .map_err(|e| format!("Failed to resolve workspace_dir: {}", e))?;

        let mut resolved = Vec::with_capacity(files.len());

        for raw in files {
            if !path_security::is_path_safe(raw) {
                return Err(format!("Unsafe file path: {}", raw));
            }

            let candidate = if Path::new(raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                Path::new(&self.workspace_dir).join(raw)
            };

            let canonical = std::fs::canonicalize(&candidate)
                .map_err(|e| format!("Failed to resolve file '{}': {}", raw, e))?;

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
