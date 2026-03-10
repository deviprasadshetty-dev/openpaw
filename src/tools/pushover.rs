use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;
use tokio::fs;

const PUSHOVER_API_URL: &str = "https://api.pushover.net/1/messages.json";

pub struct PushoverTool {
    pub workspace_dir: String,
}

#[async_trait]
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

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) if !m.is_empty() => m,
            _ => return Ok(ToolResult::fail("Missing required 'message' parameter")),
        };

        let title = args.get("title").and_then(|v| v.as_str());
        let sound = args.get("sound").and_then(|v| v.as_str());
        let priority = args.get("priority").and_then(|v| v.as_i64());

        if let Some(p) = priority
            && (!(-2..=2).contains(&p)) {
                return Ok(ToolResult::fail(
                    "Invalid 'priority': expected integer in range -2..=2",
                ));
            }

        let env_path = std::path::Path::new(&self.workspace_dir).join(".env");
        let content = fs::read_to_string(&env_path).await.unwrap_or_default();

        let mut token = String::new();
        let mut user_key = String::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = if let Some(stripped) = line.strip_prefix("export ") {
                stripped
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
                "Pushover PUSHOVER_TOKEN is missing. Please set it in your .env file.",
            ));
        }
        if user_key.is_empty() {
            return Ok(ToolResult::fail(
                "Pushover PUSHOVER_USER_KEY is missing. Please set it in your .env file.",
            ));
        }

        let mut params = HashMap::new();
        params.insert("token", token.as_str());
        params.insert("user", user_key.as_str());
        params.insert("message", message);

        if let Some(t) = title {
            params.insert("title", t);
        }
        let p_str;
        if let Some(p) = priority {
            p_str = p.to_string();
            params.insert("priority", &p_str);
        }
        if let Some(s) = sound {
            params.insert("sound", s);
        }

        let client = Client::new();
        let resp = match client.post(PUSHOVER_API_URL).form(&params).send().await {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::fail(format!("Pushover request failed: {}", e))),
        };

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if status.is_success() && body.contains("\"status\":1") {
            Ok(ToolResult::ok("Notification sent successfully"))
        } else {
            Ok(ToolResult::fail(format!(
                "Pushover API error ({}): {}",
                status, body
            )))
        }
    }
}
