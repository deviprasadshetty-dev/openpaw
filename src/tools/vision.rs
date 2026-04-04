use crate::tools::{Tool, ToolContext, ToolResult};
use crate::multimodal::{is_gemini_cli_available, process_with_gemini_cli};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct VisionTool;

#[async_trait]
impl Tool for VisionTool {
    fn name(&self) -> &str {
        "vision"
    }

    fn description(&self) -> &str {
        "Analyze images, videos, audio, or documents. Use this when you need to 'see' or 'hear' a file. If multiple files are provided, they will be analyzed together."
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

        if is_gemini_cli_available() {
            match process_with_gemini_cli(prompt, &files) {
                Ok(res) => Ok(ToolResult::ok(json!({ "analysis": res, "method": "gemini-cli" }).to_string())),
                Err(e) => Ok(ToolResult::fail(format!("Gemini CLI failed: {}", e))),
            }
        } else {
            Ok(ToolResult::ok(json!({ 
                "error": "Gemini CLI not available on this system. Please install it for high-powered multimodal processing.",
                "hint": "You can still process images if your current AI provider supports native multimodal inputs."
            }).to_string()))
        }
    }
}
