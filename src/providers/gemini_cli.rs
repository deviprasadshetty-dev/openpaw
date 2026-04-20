use anyhow::{Context, Result};
use serde::Deserialize;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};
use tracing::debug;

use crate::providers::{
    ChatMessage, ChatRequest, ChatResponse, Provider, StreamCallback, StreamChunk, TokenUsage,
};

const DEFAULT_MODEL: &str = "gemini-2.0-flash";
const CLI_NAME: &str = "gemini";
const ACP_PROTOCOL_VERSION: u32 = 1;




#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    id: Option<Value>,
    result: Option<Value>,
    error: Option<Value>,
    method: Option<String>,
    params: Option<Value>,
}

struct Inner {
    child: Option<Child>,
    session_id: Option<String>,
}

/// Provider that delegates to the `gemini` CLI (Google Gemini).
/// Creates a persistent session via JSON-RPC 2.0 (ACP).
pub struct GeminiCliProvider {
    model: String,
    inner: Arc<Mutex<Inner>>,
    next_id: AtomicU32,
}

impl GeminiCliProvider {
    pub fn new(model: Option<&str>) -> Self {
        Self {
            model: model.unwrap_or(DEFAULT_MODEL).to_string(),
            inner: Arc::new(Mutex::new(Inner {
                child: None,
                session_id: None,
            })),
            next_id: AtomicU32::new(1),
        }
    }

    fn ensure_started(&self) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.child.is_some() && inner.session_id.is_some() {
            return Ok(());
        }

        debug!("Starting Gemini CLI process...");
        
        // Ensure old process is killed if it partially failed
        if let Some(mut child) = inner.child.take() {
            let _ = child.kill();
        }

        let mut child = Command::new(CLI_NAME)
            .args(["--experimental-acp", "--approval-mode", "yolo"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("Failed to spawn gemini CLI. Ensure it is in your PATH.")?;

        let session_id = self.initialize_session(&mut child)?;
        
        inner.child = Some(child);
        inner.session_id = Some(session_id);

        Ok(())
    }

    fn initialize_session(&self, child: &mut Child) -> Result<String> {
        let mut stdin = child.stdin.take().context("Failed to open stdin")?;
        let stdout = child.stdout.take().context("Failed to open stdout")?;
        let mut reader = BufReader::new(stdout);

        let init_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let init_req = json!({
            "jsonrpc": "2.0",
            "id": init_id,
            "method": "initialize",
            "params": {
                "protocolVersion": ACP_PROTOCOL_VERSION,
                "clientCapabilities": {}
            }
        });

        Self::send_line(&mut stdin, &init_req)?;
        
        // Read initialize response
        loop {
            let line = Self::read_line(&mut reader)?;
            let resp: JsonRpcResponse = serde_json::from_str(&line)?;
            
            if resp.id.as_ref().and_then(|v| v.as_u64()) == Some(init_id as u64) {
                if let Some(err) = resp.error {
                    anyhow::bail!("ACP initialize failed: {}", err);
                }
                break;
            }
        }

        // session/new
        let cwd = std::env::current_dir()?.to_string_lossy().to_string();
        let session_id_rpc = self.next_id.fetch_add(1, Ordering::SeqCst);
        let session_req = json!({
            "jsonrpc": "2.0",
            "id": session_id_rpc,
            "method": "session/new",
            "params": {
                "cwd": cwd,
                "mcpServers": []
            }
        });

        Self::send_line(&mut stdin, &session_req)?;

        let sid = loop {
            let line = Self::read_line(&mut reader)?;
            let resp: JsonRpcResponse = serde_json::from_str(&line)?;

            if resp.id.as_ref().and_then(|v| v.as_u64()) == Some(session_id_rpc as u64) {
                if let Some(err) = resp.error {
                    anyhow::bail!("ACP session/new failed: {}", err);
                }
                let res = resp.result.context("No result in session/new response")?;
                let sid = res.get("sessionId")
                    .and_then(|v| v.as_str())
                    .context("No sessionId in result")?;
                break sid.to_string();
            }
        };

        // Put stdin/stdout back (using original handle)
        child.stdin = Some(stdin);
        child.stdout = Some(reader.into_inner());

        Ok(sid)
    }

    fn send_line<W: Write>(writer: &mut W, val: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(val)?;
        line.push(b'\n');
        writer.write_all(&line)?;
        writer.flush()?;
        Ok(())
    }

    fn read_line<R: BufRead>(reader: &mut R) -> Result<String> {
        let mut line = String::new();
        // Use a loop to skip non-JSON lines or prefixes
        loop {
            line.clear();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                anyhow::bail!("End of stream while waiting for JSON line");
            }
            let trimmed = line.trim();
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                return Ok(line);
            }
        }
    }

    fn extract_last_user_message(messages: &[ChatMessage]) -> Option<&str> {
        messages.iter().rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
    }
}

impl Provider for GeminiCliProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        self.ensure_started()?;
        let mut inner = self.inner.lock().unwrap();
        let session_id = inner.session_id.clone().unwrap();
        let child = inner.child.as_mut().unwrap();
        let mut stdin = child.stdin.as_mut().unwrap();
        let stdout = child.stdout.take().context("Stdout already taken")?;
        let mut reader = BufReader::new(stdout);

        let prompt = Self::extract_last_user_message(request.messages)
            .context("No user message found in request")?;

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "model": self.model,
                "prompt": [
                    { "type": "text", "text": prompt }
                ]
            }
        });

        Self::send_line(&mut stdin, &req)?;

        let mut full_content = String::new();

        let res: Result<()> = loop {
            let line_res = Self::read_line(&mut reader);
            let line = match line_res {
                Ok(l) => l,
                Err(e) => {
                    child.stdout = Some(reader.into_inner());
                    return Err(e);
                }
            };
            let resp: JsonRpcResponse = serde_json::from_str(&line)?;

            // Handle session/update (delta)
            if let Some(method) = resp.method.as_deref()
                && method == "session/update"
            {
                if let Some(update) = resp.params.and_then(|p| p.get("update").cloned())
                    && let Some(content) = update.get("content")
                    && let Some(text) = content.get("text").and_then(|v| v.as_str())
                {
                    full_content.push_str(text);
                }
                continue;
            }

            // Handle final response
            if resp.id.as_ref().and_then(|v| v.as_u64()) == Some(id as u64) {
                if let Some(err) = resp.error {
                    child.stdout = Some(reader.into_inner());
                    anyhow::bail!("ACP session/prompt failed: {}", err);
                }
                
                if let Some(res) = resp.result
                    && let Some(c) = res.get("content").and_then(|v| v.as_str())
                    && !c.is_empty()
                {
                    full_content = c.to_string();
                }
                break Ok(());
            }
        };

        // Return original stdout to child handle
        child.stdout = Some(reader.into_inner());
        res?;

        Ok(ChatResponse {
            content: if full_content.is_empty() { None } else { Some(full_content) },
            tool_calls: vec![],
            usage: TokenUsage::default(),
            model: self.model.clone(),
            reasoning_content: None,
            thought_signature: None,
        })
    }

    fn chat_stream(&self, request: &ChatRequest, mut callback: StreamCallback) -> Result<ChatResponse> {
        self.ensure_started()?;
        let mut inner = self.inner.lock().unwrap();
        let session_id = inner.session_id.clone().unwrap();
        let child = inner.child.as_mut().unwrap();
        let mut stdin = child.stdin.as_mut().unwrap();
        let stdout = child.stdout.take().context("Stdout already taken")?;
        let mut reader = BufReader::new(stdout);

        let prompt = Self::extract_last_user_message(request.messages)
            .context("No user message found in request")?;

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "model": self.model,
                "prompt": [
                    { "type": "text", "text": prompt }
                ]
            }
        });

        Self::send_line(&mut stdin, &req)?;

        let mut full_content = String::new();

        let res: Result<()> = loop {
            let line_res = Self::read_line(&mut reader);
            let line = match line_res {
                Ok(l) => l,
                Err(e) => {
                    child.stdout = Some(reader.into_inner());
                    return Err(e);
                }
            };
            let resp: JsonRpcResponse = serde_json::from_str(&line)?;

            // Handle session/update (delta)
            if let Some(method) = resp.method.as_deref()
                && method == "session/update"
            {
                if let Some(update) = resp.params.and_then(|p| p.get("update").cloned())
                    && let Some(content) = update.get("content")
                    && let Some(text) = content.get("text").and_then(|v| v.as_str())
                {
                    full_content.push_str(text);
                    callback(StreamChunk::Delta(text.to_string()));
                }
                continue;
            }

            // Handle final response
            if resp.id.as_ref().and_then(|v| v.as_u64()) == Some(id as u64) {
                if let Some(err) = resp.error {
                    child.stdout = Some(reader.into_inner());
                    anyhow::bail!("ACP session/prompt failed: {}", err);
                }
                break Ok(());
            }
        };

        callback(StreamChunk::Done(TokenUsage::default()));
        child.stdout = Some(reader.into_inner());
        res?;

        Ok(ChatResponse {
            content: if full_content.is_empty() { None } else { Some(full_content) },
            tool_calls: vec![],
            usage: TokenUsage::default(),
            model: self.model.clone(),
            reasoning_content: None,
            thought_signature: None,
        })
    }

    fn supports_native_tools(&self) -> bool {
        false
    }

    fn get_name(&self) -> &str {
        "gemini-cli"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ChatMessage;

    #[test]
    fn test_extract_last_user_message() {
        let msgs = vec![
            ChatMessage::system("Be helpful"),
            ChatMessage::user("first"),
            ChatMessage::assistant("ok"),
            ChatMessage::user("second"),
        ];
        let result = GeminiCliProvider::extract_last_user_message(&msgs);
        assert_eq!(result, Some("second"));
    }

    #[test]
    fn test_extract_last_user_message_empty() {
        let msgs: Vec<ChatMessage> = vec![];
        let result = GeminiCliProvider::extract_last_user_message(&msgs);
        assert_eq!(result, None);
    }
}
