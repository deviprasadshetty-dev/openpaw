use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::tools::Tool;
use crate::config_types::McpServerConfig;
use crate::version;

// ── Tool definition from server ─────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

// ── McpServer ───────────────────────────────────────────────────

pub struct McpServer {
    pub name: String,
    pub config: McpServerConfig,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    stdout: Mutex<Option<BufReader<ChildStdout>>>,
    next_id: AtomicU32,
}

impl McpServer {
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            name: config.name.clone(),
            config,
            child: Mutex::new(None),
            stdin: Mutex::new(None),
            stdout: Mutex::new(None),
            next_id: AtomicU32::new(1),
        }
    }

    pub fn connect(&self) -> Result<()> {
        let mut cmd = Command::new(&self.config.command);
        cmd.args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        // Setup environment
        for env_entry in &self.config.env {
            cmd.env(&env_entry.key, &env_entry.value);
        }

        let mut child = cmd.spawn().context("Failed to spawn MCP server")?;

        let stdin = child.stdin.take().context("Failed to attach to stdin")?;
        let stdout = child.stdout.take().context("Failed to attach to stdout")?;

        *self.child.lock().unwrap() = Some(child);
        *self.stdin.lock().unwrap() = Some(stdin);
        *self.stdout.lock().unwrap() = Some(BufReader::new(stdout));

        // Send initialize request
        let init_params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "openpaw",
                "version": version::VERSION
            }
        });

        let init_resp_str = self.send_request("initialize", Some(init_params))?;
        let init_resp: Value = serde_json::from_str(&init_resp_str)?;

        if init_resp
            .get("result")
            .and_then(|r| r.get("protocolVersion"))
            .is_none()
        {
            return Err(anyhow::anyhow!("Invalid handshake response"));
        }

        // Send initialized notification
        self.send_notification("notifications/initialized", None)?;

        Ok(())
    }

    pub fn list_tools(&self) -> Result<Vec<McpToolDef>> {
        let resp_str = self.send_request("tools/list", Some(serde_json::json!({})))?;
        let resp: Value = serde_json::from_str(&resp_str)?;

        if let Some(err) = resp.get("error") {
            return Err(anyhow::anyhow!("JSON-RPC Error: {}", err));
        }

        let result = resp.get("result").context("Missing result")?;
        let tools_val = result.get("tools").context("Missing tools in result")?;

        let tools: Vec<McpToolDef> = serde_json::from_value(tools_val.clone())?;
        Ok(tools)
    }

    pub fn call_tool(&self, tool_name: &str, args: &Value) -> Result<String> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": args
        });

        let resp_str = self.send_request("tools/call", Some(params))?;
        let resp: Value = serde_json::from_str(&resp_str)?;

        if let Some(err) = resp.get("error") {
            return Err(anyhow::anyhow!("JSON-RPC Error: {}", err));
        }

        let result = resp.get("result").context("Missing result")?;
        let content_arr = result.get("content").context("Missing content")?;

        let mut output = String::new();
        if let Some(arr) = content_arr.as_array() {
            for item in arr {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(text);
                }
            }
        }

        Ok(output)
    }

    fn send_request(&self, method: &str, params: Option<Value>) -> Result<String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
        });
        if let Some(p) = params {
            req.as_object_mut().unwrap().insert("params".to_string(), p);
        }

        let mut msg_str = serde_json::to_string(&req)?;
        msg_str.push('\n');

        {
            let mut stdin_guard = self.stdin.lock().unwrap();
            let stdin = stdin_guard.as_mut().context("No stdin")?;
            stdin.write_all(msg_str.as_bytes())?;
            stdin.flush()?;
        }

        self.read_line()
    }

    fn send_notification(&self, method: &str, params: Option<Value>) -> Result<()> {
        let mut req = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
        });
        if let Some(p) = params {
            req.as_object_mut().unwrap().insert("params".to_string(), p);
        }

        let mut msg_str = serde_json::to_string(&req)?;
        msg_str.push('\n');

        {
            let mut stdin_guard = self.stdin.lock().unwrap();
            let stdin = stdin_guard.as_mut().context("No stdin")?;
            stdin.write_all(msg_str.as_bytes())?;
            stdin.flush()?;
        }

        Ok(())
    }

    fn read_line(&self) -> Result<String> {
        let mut line = String::new();
        let mut stdout_guard = self.stdout.lock().unwrap();
        let stdout = stdout_guard.as_mut().context("No stdout")?;
        let bytes_read = stdout.read_line(&mut line)?;
        if bytes_read == 0 {
            return Err(anyhow::anyhow!("End of stream"));
        }
        Ok(line.trim_end().to_string())
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        if let Ok(mut child_guard) = self.child.lock() {
            if let Some(mut child) = child_guard.take() {
                let _ = self.stdin.lock().unwrap().take();
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

// ── McpToolWrapper ──────────────────────────────────────────────

pub struct McpToolWrapper {
    server: Arc<McpServer>,
    original_name: String,
    prefixed_name: String,
    desc: String,
    params_json_str: String,
}

impl Tool for McpToolWrapper {
    fn name(&self) -> &str {
        &self.prefixed_name
    }

    fn description(&self) -> &str {
        &self.desc
    }

    fn parameters_json(&self) -> &str {
        &self.params_json_str
    }

    fn execute(&self, args: &Value) -> Result<String> {
        match self.server.call_tool(&self.original_name, args) {
            Ok(output) => Ok(output),
            Err(e) => Ok(format!("MCP tool '{}' failed: {}", self.original_name, e)),
        }
    }
}

// ── Top-level init ──────────────────────────────────────────────

pub fn init_mcp_tools(configs: &[McpServerConfig]) -> Result<Vec<Arc<dyn Tool>>> {
    let mut all_tools: Vec<Arc<dyn Tool>> = Vec::new();

    for cfg in configs {
        let server = Arc::new(McpServer::new(cfg.clone()));

        if let Err(e) = server.connect() {
            tracing::error!("MCP server '{}': connect failed: {}", cfg.name, e);
            continue;
        }

        let tool_defs = match server.list_tools() {
            Ok(defs) => defs,
            Err(e) => {
                tracing::error!("MCP server '{}': tools/list failed: {}", cfg.name, e);
                continue;
            }
        };

        let mut transferred = 0;
        for td in tool_defs {
            let prefixed_name = format!("mcp_{}_{}", cfg.name, td.name);
            let desc = td.description.unwrap_or_default();
            let params_json_str =
                serde_json::to_string(&td.input_schema).unwrap_or_else(|_| "{}".to_string());

            let wrapper = McpToolWrapper {
                server: Arc::clone(&server),
                original_name: td.name,
                prefixed_name,
                desc,
                params_json_str,
            };

            all_tools.push(Arc::new(wrapper));
            transferred += 1;
        }

        tracing::info!(
            "MCP server '{}': {} tools registered",
            cfg.name,
            transferred
        );
    }

    Ok(all_tools)
}
