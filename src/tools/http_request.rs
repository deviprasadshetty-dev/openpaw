use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use reqwest::Url;
use reqwest::blocking::Client;
use serde_json::Value;
use std::time::Duration;

pub struct HttpRequestTool {
    pub max_response_size: usize,
    pub timeout_secs: u64,
    pub allowed_domains: Vec<String>,
}

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

    fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return Ok(ToolResult::fail("Missing 'url' parameter")),
        };

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

        if let Some(h_val) = headers_json {
            if let Some(h_map) = h_val.as_object() {
                for (k, v) in h_map {
                    if let Some(v_str) = v.as_str() {
                        req = req.header(k, v_str);
                    }
                }
            }
        }

        if let Some(b) = body_str {
            req = req.body(b.to_string());
        }

        let resp = match req.send() {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::fail(format!("Request failed: {}", e))),
        };

        let status = resp.status();
        let success = status.is_success();

        // Read body with limit
        let text = match resp.text() {
            Ok(t) => {
                if t.len() > self.max_response_size {
                    format!("{}\n\n[Content truncated]", &t[..self.max_response_size])
                } else {
                    t
                }
            }
            Err(e) => {
                return Ok(ToolResult::fail(format!(
                    "Failed to read response body: {}",
                    e
                )));
            }
        };

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
