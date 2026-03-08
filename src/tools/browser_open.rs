use super::{Tool, ToolContext, ToolResult, process_util};
use anyhow::Result;
use serde_json::Value;

pub struct BrowserOpenTool {
    pub allowed_domains: Vec<String>,
}

impl Tool for BrowserOpenTool {
    fn name(&self) -> &str {
        "browser_open"
    }

    fn description(&self) -> &str {
        "Open an approved HTTPS URL in the default browser. Only allowlisted domains are permitted."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"url":{"type":"string","description":"HTTPS URL to open in browser"}},"required":["url"]}"#.to_string()
    }

    fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return Ok(ToolResult::fail("Missing 'url' parameter")),
        };

        if !url.starts_with("https://") {
            return Ok(ToolResult::fail("Only https:// URLs are allowed"));
        }

        // Basic domain extraction
        let host = url
            .split("://")
            .nth(1)
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("")
            .split(':')
            .next()
            .unwrap_or("");

        if host.is_empty() {
            return Ok(ToolResult::fail("URL must include a host"));
        }

        // Block local
        if is_local_or_private(host) {
            return Ok(ToolResult::fail("Blocked local/private host"));
        }

        if self.allowed_domains.is_empty() {
            return Ok(ToolResult::fail(
                "No allowed_domains configured for browser_open",
            ));
        }

        if !host_matches_allowlist(host, &self.allowed_domains) {
            return Ok(ToolResult::fail("Host is not in browser allowed_domains"));
        }

        #[cfg(target_os = "macos")]
        let argv = vec!["open", url];
        #[cfg(target_os = "linux")]
        let argv = vec!["xdg-open", url];
        #[cfg(windows)]
        let argv = vec!["cmd.exe", "/c", "start", url]; // Needs sanitization like browser tool
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        return Ok(ToolResult::fail(
            "browser_open not supported on this platform",
        ));

        let result = process_util::run(&argv, process_util::RunOptions::default())?;

        if result.success {
            Ok(ToolResult::ok(format!("Opened in browser: {}", url)))
        } else {
            Ok(ToolResult::fail("Browser command failed"))
        }
    }
}

fn is_local_or_private(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host == "::1"
        || host.starts_with("127.")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.starts_with("169.254.")
}

fn host_matches_allowlist(host: &str, allowed: &[String]) -> bool {
    for domain in allowed {
        if host == domain {
            return true;
        }
        if host.ends_with(domain)
            && host.len() > domain.len()
            && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
        {
            return true;
        }
    }
    false
}
