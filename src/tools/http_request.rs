use super::{Tool, ToolContext, ToolResult, ssrf_guard};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::{Client, Url};
use serde_json::Value;
use std::time::Duration;

pub struct HttpRequestTool {
    pub max_response_size: usize,
    pub timeout_secs: u64,
    pub allowed_domains: Vec<String>,
}

#[async_trait]
impl Tool for HttpRequestTool {
    fn name(&self) -> &str {
        "http_request"
    }

    fn description(&self) -> &str {
        "Make HTTP requests (GET, POST, etc.)"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"url":{"type":"string","description":"URL to request"},"method":{"type":"string","description":"HTTP method (GET, POST, etc.)","default":"GET"},"headers":{"type":"object","description":"HTTP headers"},"body":{"type":"string","description":"Request body"}},"required":["url"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return Ok(ToolResult::fail("Missing 'url' parameter")),
        };

        // ── 0.1: SSRF protection ─────────────────────────────────────────────
        if let Err(e) = ssrf_guard::check_url(url).await {
            return Ok(ToolResult::fail(format!("Security: {}", e)));
        }

        // ── Optional domain allowlist ─────────────────────────────────────────
        if !self.allowed_domains.is_empty() {
            let host = match Url::parse(url) {
                Ok(u) => u.host_str().map(|h| h.to_string()),
                Err(_) => return Ok(ToolResult::fail("Invalid URL")),
            };

            if let Some(h) = host {
                let allowed = self
                    .allowed_domains
                    .iter()
                    .any(|d| h == *d || h.ends_with(&format!(".{}", d)));
                if !allowed {
                    return Ok(ToolResult::fail(format!(
                        "Domain {} not in allowed list",
                        h
                    )));
                }
            } else {
                return Ok(ToolResult::fail("Could not parse host from URL"));
            }
        }

        let method = args.get("method").and_then(|v| v.as_str()).unwrap_or("GET");
        let headers_json = args.get("headers");
        let body_str = args.get("body").and_then(|v| v.as_str());

        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()?;

        let mut req = match method.to_uppercase().as_str() {
            "GET" => client.get(url),
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "DELETE" => client.delete(url),
            "PATCH" => client.patch(url),
            "HEAD" => client.head(url),
            _ => return Ok(ToolResult::fail(format!("Unsupported method: {}", method))),
        };

        // ── 0.8: CRLF injection validation ───────────────────────────────────
        if let Some(h_val) = headers_json
            && let Some(h_map) = h_val.as_object() {
                for (k, v) in h_map {
                    // Reject header name or value containing CRLF characters
                    if k.contains('\r') || k.contains('\n') {
                        return Ok(ToolResult::fail(format!(
                            "Header name '{}' contains invalid characters (CRLF)", k
                        )));
                    }
                    if let Some(v_str) = v.as_str() {
                        if v_str.contains('\r') || v_str.contains('\n') {
                            return Ok(ToolResult::fail(format!(
                                "Header '{}' value contains invalid characters (CRLF)", k
                            )));
                        }
                        req = req.header(k, v_str);
                    }
                }
            }

        if let Some(b) = body_str {
            req = req.body(b.to_string());
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::fail(format!("Request failed: {}", e))),
        };

        let status = resp.status();
        let success = status.is_success();

        if let Some(content_length) = resp.content_length() {
            if content_length > (self.max_response_size as u64) {
                return Ok(ToolResult::fail(format!(
                    "HTTP {}: Response too large (Content-Length: {} bytes, limit: {} bytes)",
                    status, content_length, self.max_response_size
                )));
            }
        }

        let mut text = String::new();
        let mut total_size = 0;
        let mut truncated = false;

        let mut resp = resp;
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    total_size += chunk.len();
                    if total_size > self.max_response_size {
                        truncated = true;
                        break;
                    }
                    text.push_str(&String::from_utf8_lossy(&chunk));
                }
                Ok(None) => break,
                Err(e) => return Ok(ToolResult::fail(format!("Failed to read response body: {}", e))),
            }
        }

        if truncated {
            text.push_str("\n\n[Content truncated]");
        }

        if success {
            Ok(ToolResult::ok(format!(
                "Status: {}\n\nResponse Body:\n{}",
                status, text
            )))
        } else {
            Ok(ToolResult::fail(format!("HTTP {}: {}", status, text)))
        }
    }
}
