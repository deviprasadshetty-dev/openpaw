use super::{Tool, ToolResult};
use crate::tools::process_util;
use anyhow::Result;
use serde_json::Value;

pub struct ComposioTool {
    pub api_key: String,
    pub entity_id: String,
}

impl Tool for ComposioTool {
    fn name(&self) -> &str {
        "composio"
    }

    fn description(&self) -> &str {
        "Execute actions on 1000+ apps via Composio (Gmail, Notion, GitHub, Slack, etc.). Use action='list', 'execute', or 'connect'."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"action":{"type":"string","enum":["list","execute","connect"],"description":"Operation to perform"},"app":{"type":"string","description":"App name for listing actions (e.g. 'github')"},"action_name":{"type":"string","description":"Action to execute (e.g. 'github_star_repo')"},"params":{"type":"object","description":"Parameters for the action"},"entity_id":{"type":"string","description":"Optional entity override"}},"required":["action"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return Ok(ToolResult::fail("Missing 'action' parameter")),
        };

        match action {
            "list" => self.list_actions(&args),
            "execute" => self.execute_action(&args),
            "connect" => Ok(ToolResult::fail("Connect flow not supported in headless mode yet")),
            _ => Ok(ToolResult::fail(format!("Unknown action: {}", action))),
        }
    }
}

impl ComposioTool {
    fn run_curl(&self, args: &[&str]) -> Result<ToolResult> {
        let api_key_header = format!("x-api-key: {}", self.api_key);
        let mut argv = vec!["curl", "-sL", "-m", "15", "-H", &api_key_header];
        argv.extend_from_slice(args);
        
        let result = process_util::run(&argv, process_util::RunOptions::default())?;
        
        if result.success {
             if !result.stdout.is_empty() {
                 Ok(ToolResult::ok(result.stdout))
             } else {
                 Ok(ToolResult::ok("(empty response)"))
             }
        } else {
             Ok(ToolResult::fail(format!("curl failed: {}", result.stderr)))
        }
    }

    fn list_actions(&self, args: &Value) -> Result<ToolResult> {
        let app = args.get("app").and_then(|v| v.as_str());
        let url = if let Some(a) = app {
            format!("https://backend.composio.dev/api/v3/tools?toolkits={}&page=1&page_size=100", a)
        } else {
            "https://backend.composio.dev/api/v3/tools?page=1&page_size=100".to_string()
        };
        self.run_curl(&["-X", "GET", &url])
    }

    fn execute_action(&self, args: &Value) -> Result<ToolResult> {
        let action_name = args.get("tool_slug").or(args.get("action_name")).and_then(|v| v.as_str());
        let action = match action_name {
             Some(a) => a,
             None => return Ok(ToolResult::fail("Missing 'action_name' or 'tool_slug'")),
        };
        
        let entity_id = args.get("entity_id").and_then(|v| v.as_str()).unwrap_or(&self.entity_id);
        
        // Build body... skipped complex json building for brevity, using simple string construction or serde
        // Since I'm using curl, I need to pass body string.
        let default_params = serde_json::json!({});
        let params = args.get("params").unwrap_or(&default_params);
        let body_json = serde_json::json!({
            "arguments": params,
            "user_id": entity_id,
            "connected_account_id": args.get("connected_account_id")
        });
        
        let url = format!("https://backend.composio.dev/api/v3/tools/{}/execute", action);
        self.run_curl(&["-X", "POST", "-H", "Content-Type: application/json", "-d", &body_json.to_string(), &url])
    }
}
