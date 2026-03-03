use super::{process_util, Tool, ToolResult};
use anyhow::Result;
use serde_json::Value;

pub struct ScreenshotTool {
    pub workspace_dir: String,
}

impl Tool for ScreenshotTool {
    fn name(&self) -> &str {
        "screenshot"
    }

    fn description(&self) -> &str {
        "Capture a screenshot of the current screen. Returns [IMAGE:path] marker — include it verbatim in your response to send the image to the user."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"filename":{"type":"string","description":"Optional filename (default: screenshot.png). Saved in workspace."}}}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let filename = args.get("filename").and_then(|v| v.as_str()).unwrap_or("screenshot.png");
        
        // Basic path sanitization for filename
        if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
             return Ok(ToolResult::fail("Invalid filename"));
        }

        let output_path = format!("{}/{}", self.workspace_dir, filename);

        #[cfg(target_os = "macos")]
        {
            let cmd_args = vec!["screencapture", "-x", &output_path];
            let result = process_util::run(&cmd_args, process_util::RunOptions::default())?;
            
            if result.success {
                 Ok(ToolResult::ok(format!("[IMAGE:{}]", output_path)))
            } else {
                 let err_msg = if !result.stderr.is_empty() { result.stderr } else { "unknown error".to_string() };
                 Ok(ToolResult::fail(format!("Screenshot command failed: {}", err_msg)))
            }
        }
        
        #[cfg(target_os = "linux")]
        {
            let cmd_args = vec!["import", "-window", "root", &output_path];
            let result = process_util::run(&cmd_args, process_util::RunOptions::default())?;
            
            if result.success {
                 Ok(ToolResult::ok(format!("[IMAGE:{}]", output_path)))
            } else {
                 let err_msg = if !result.stderr.is_empty() { result.stderr } else { "unknown error".to_string() };
                 Ok(ToolResult::fail(format!("Screenshot command failed: {}", err_msg)))
            }
        }
        
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Ok(ToolResult::fail("Screenshot not supported on this platform"))
        }
    }
}
