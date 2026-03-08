use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const PUSHOVER_API_URL: &str = "https://api.pushover.net/1/messages.json";

pub struct PushoverTool {
    pub workspace_dir: String,
}

impl Tool for PushoverTool {
    fn name(&self) -> &str {
        "pushover"
    }

    fn description(&self) -> &str {
        "Send a push notification via Pushover. Requires PUSHOVER_TOKEN and PUSHOVER_USER_KEY in .env file."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"message":{"type":"string","description":"The notification message"},"title":{"type":"string","description":"Optional title"},"priority":{"type":"integer","description":"Priority -2..2 (default 0)"},"sound":{"type":"string","description":"Optional sound name"}},"required":["message"]}"#.to_string()
    }

    fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) if !m.is_empty() => m,
            _ => return Ok(ToolResult::fail("Missing required 'message' parameter")),
        };

        let title = args.get("title").and_then(|v| v.as_str());
        let sound = args.get("sound").and_then(|v| v.as_str());
        let priority = args.get("priority").and_then(|v| v.as_i64());

        if let Some(p) = priority {
            if p < -2 || p > 2 {
                return Ok(ToolResult::fail(
                    "Invalid 'priority': expected integer in range -2..=2",
                ));
            }
        }

        let env_path = PathBuf::from(&self.workspace_dir).join(".env");
        let content = fs::read_to_string(&env_path).unwrap_or_default();

        let mut token = String::new();
        let mut user_key = String::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = if line.starts_with("export ") {
                &line[7..]
            } else {
                line
            };

            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim();
                let value = v.trim().trim_matches(|c| c == '"' || c == '\'');
                if key == "PUSHOVER_TOKEN" {
                    token = value.to_string();
                } else if key == "PUSHOVER_USER_KEY" {
                    user_key = value.to_string();
                }
            }
        }

        if token.is_empty() {
            return Ok(ToolResult::fail(
                "Pushover PUSHOVER_TOKEN is missing. Please set it in your .env file or run 'openpaw onboard' to configure it.",
            ));
        }
        if user_key.is_empty() {
            return Ok(ToolResult::fail(
                "Pushover PUSHOVER_USER_KEY is missing. Please set it in your .env file or run 'openpaw onboard' to configure it.",
            ));
        }

        let mut body = format!("token={}&user={}&message={}", token, user_key, message);
        if let Some(t) = title {
            body.push_str(&format!("&title={}", t));
        }
        if let Some(p) = priority {
            body.push_str(&format!("&priority={}", p));
        }
        if let Some(s) = sound {
            body.push_str(&format!("&sound={}", s));
        }

        let output = Command::new("curl")
            .args(&["-s", "-X", "POST", "-d", &body, PUSHOVER_API_URL])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("\"status\":1") {
            Ok(ToolResult::ok("Notification sent successfully"))
        } else {
            Ok(ToolResult::fail("Pushover API returned an error"))
        }
    }
}
