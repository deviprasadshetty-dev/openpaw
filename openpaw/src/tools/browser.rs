use super::{process_util, Tool, ToolResult};
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

const MAX_READ_BYTES: usize = 8192;
const MAX_FETCH_BYTES: usize = 65536;

pub struct BrowserTool;

impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Browse web pages. Actions: open, screenshot, click, type, scroll, read."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"action":{"type":"string","enum":["open","screenshot","click","type","scroll","read"],"description":"Browser action to perform"},"url":{"type":"string","description":"URL to open"},"selector":{"type":"string","description":"CSS selector"},"text":{"type":"string","description":"Text to type"}},"required":["action"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return Ok(ToolResult::fail("Missing 'action' parameter")),
        };

        match action {
            "open" => Self::execute_open(args),
            "read" => Self::execute_read(args),
            "screenshot" => Ok(ToolResult::fail("Use the screenshot tool instead")),
            "click" | "type" | "scroll" => {
                let msg = format!("Browser action '{}' requires CDP which is not available. Use 'open' to launch in system browser or 'read' to fetch page content.", action);
                Ok(ToolResult::fail(msg))
            }
            _ => Ok(ToolResult::fail(format!("Unknown browser action '{}'", action))),
        }
    }
}

impl BrowserTool {
    fn execute_open(args: Value) -> Result<ToolResult> {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return Ok(ToolResult::fail("Missing 'url' parameter for open action")),
        };

        if !url.starts_with("https://") {
             return Ok(ToolResult::fail("Only https:// URLs are supported for security"));
        }

        // Basic shell safety check for Windows
        #[cfg(windows)]
        {
            if url.contains(&['&', '|', ';', '"', '\'', '<', '>', '`', '(', ')', '^', '%', '!', '\n', '\r'][..]) {
                 return Ok(ToolResult::fail("URL contains shell metacharacters"));
            }
        }

        #[cfg(target_os = "macos")]
        let argv = vec!["open", url];
        #[cfg(target_os = "linux")]
        let argv = vec!["xdg-open", url];
        #[cfg(windows)]
        let argv = vec!["cmd.exe", "/c", "start", url];
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        return Ok(ToolResult::fail("Browser open not supported on this platform"));

        let result = process_util::run(&argv, process_util::RunOptions::default())?;

        if result.success {
             Ok(ToolResult::ok(format!("Opened {} in system browser", url)))
        } else {
             Ok(ToolResult::fail(format!("Browser open command exited with code {:?}", result.exit_code)))
        }
    }

    fn execute_read(args: Value) -> Result<ToolResult> {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return Ok(ToolResult::fail("Missing 'url' parameter for read action")),
        };

        let max_size_str = MAX_FETCH_BYTES.to_string();
        let argv = vec!["curl", "-sS", "-L", "-m", "10", "--max-filesize", &max_size_str, url];
        
        let mut opts = process_util::RunOptions::default();
        opts.max_output_bytes = MAX_FETCH_BYTES;

        let result = process_util::run(&argv, opts)?;

        if !result.success {
             return Ok(ToolResult::fail(format!("curl failed: {}", result.stderr)));
        }

        if result.stdout.is_empty() {
             return Ok(ToolResult::ok("Page returned empty response"));
        }

        let truncated = result.stdout.len() > MAX_READ_BYTES;
        let body_len = if truncated { MAX_READ_BYTES } else { result.stdout.len() };
        let suffix = if truncated { "\n\n[Content truncated to 8 KB]" } else { "" };
        
        Ok(ToolResult::ok(format!("{}{}", &result.stdout[..body_len], suffix)))
    }
}
