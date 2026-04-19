/// Anthropic Claude provider — Messages API.
///
/// Supports streaming (SSE) for tool use detection, text generation,
/// and vision (base64 image blocks).
use anyhow::{Context, Result};
use base64::Engine;
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::time::Duration;
use tracing::debug;

use crate::providers::{
    ChatRequest, ChatResponse, FunctionCall, Provider, TokenUsage, ToolCall, ToolSpec,
};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 8096;

pub struct AnthropicProvider {
    pub api_key: String,
    pub client: Client,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .expect("Failed to build HTTP client"),
        }
    }

    /// Resolve API key: explicit > ANTHROPIC_API_KEY env var
    pub fn resolve_key(explicit: Option<&str>) -> String {
        if let Some(k) = explicit
            && !k.is_empty()
        {
            return k.to_string();
        }
        std::env::var("ANTHROPIC_API_KEY").unwrap_or_default()
    }
}

impl Provider for AnthropicProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        // ── Build system prompt ──────────────────────────────────
        let mut system_text = String::new();
        let mut messages: Vec<Value> = Vec::new();

        for msg in request.messages {
            match msg.role.as_str() {
                "system" => {
                    if !system_text.is_empty() {
                        system_text.push('\n');
                    }
                    system_text.push_str(&msg.content);
                }
                role => {
                    // Build content array or plain string
                    let content = if let Some(parts) = &msg.content_parts {
                        build_content_blocks(parts)
                    } else {
                        json!(msg.content)
                    };
                    messages.push(json!({
                        "role": role,
                        "content": content,
                    }));
                }
            }
        }

        // ── Build request body ───────────────────────────────────
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

        let mut body = json!({
            "model": request.model,
            "max_tokens": max_tokens,
            "messages": messages,
        });

        if !system_text.is_empty() {
            body["system"] = json!(system_text);
        }

        if request.temperature > 0.0 {
            body["temperature"] = json!(request.temperature);
        }

        // ── Tools ────────────────────────────────────────────────
        if let Some(tools) = request.tools
            && !tools.is_empty()
        {
            body["tools"] = json!(tools.iter().map(anthropic_tool).collect::<Vec<_>>());
            body["tool_choice"] = json!({"type": "auto"});
        }

        debug!("Anthropic request to model={}", request.model);

        // ── HTTP call ────────────────────────────────────────────
        let res = self
            .client
            .post(ANTHROPIC_API_URL)
            .timeout(Duration::from_secs(request.timeout_secs))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .context("Anthropic HTTP request failed")?;

        let status = res.status();
        if !status.is_success() {
            let body_text = res.text().unwrap_or_default();
            anyhow::bail!("Anthropic API error {}: {}", status.as_u16(), body_text);
        }

        let parsed: Value = res.json().context("Failed to parse Anthropic response")?;
        parse_response(parsed, request.model)
    }

    fn supports_native_tools(&self) -> bool {
        true
    }

    fn get_name(&self) -> &str {
        "anthropic"
    }
}

/// Build Anthropic tool definition from our ToolSpec.
fn anthropic_tool(t: &ToolSpec) -> Value {
    json!({
        "name": t.name,
        "description": t.description,
        "input_schema": t.parameters,
    })
}

/// Build content blocks for multimodal messages.
fn build_content_blocks(parts: &[crate::providers::ContentPart]) -> Value {
    use crate::providers::ContentPart;
    let blocks: Vec<Value> = parts
        .iter()
        .map(|p| match p {
            ContentPart::Text(t) => json!({"type": "text", "text": t}),
            ContentPart::ImageBase64 { data, media_type } => json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": media_type,
                    "data": data,
                }
            }),
            ContentPart::ImageUrl { url } => json!({
                "type": "image",
                "source": {
                    "type": "url",
                    "url": url,
                }
            }),
            ContentPart::Media { mime_type, data } => json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": mime_type,
                    "data": base64::engine::general_purpose::STANDARD.encode(data),
                }
            }),
        })
        .collect();
    json!(blocks)
}

/// Parse the Anthropic Messages API response into our common ChatResponse.
fn parse_response(v: Value, model: &str) -> Result<ChatResponse> {
    let content_arr = v["content"]
        .as_array()
        .context("Missing 'content' array in Anthropic response")?;

    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    for block in content_arr {
        match block["type"].as_str().unwrap_or("") {
            "text" => {
                if let Some(t) = block["text"].as_str() {
                    text_parts.push(t.to_string());
                }
            }
            "tool_use" => {
                let id = block["id"].as_str().unwrap_or("").to_string();
                let name = block["name"].as_str().unwrap_or("").to_string();
                let input = block["input"].clone();
                let arguments = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
                tool_calls.push(ToolCall {
                    id,
                    kind: "function".to_string(),
                    function: FunctionCall {
                        name,
                        arguments,
                        thought_signature: None,
                    },
                });
            }
            _ => {}
        }
    }

    let content = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("\n"))
    };

    // Usage
    let usage_val = &v["usage"];
    let usage = TokenUsage {
        prompt_tokens: usage_val["input_tokens"].as_u64().unwrap_or(0) as u32,
        completion_tokens: usage_val["output_tokens"].as_u64().unwrap_or(0) as u32,
        total_tokens: (usage_val["input_tokens"].as_u64().unwrap_or(0)
            + usage_val["output_tokens"].as_u64().unwrap_or(0)) as u32,
    };

    let model_str = v["model"].as_str().unwrap_or(model).to_string();

    Ok(ChatResponse {
        content,
        tool_calls,
        usage,
        model: model_str,
        reasoning_content: None,
        thought_signature: None,
    })
}
