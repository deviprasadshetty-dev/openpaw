use crate::providers::{
    ChatMessage, ChatRequest, ChatResponse, ContentPart, FunctionCall, Provider, StreamCallback,
    StreamChunk, TokenUsage, ToolCall,
};
use anyhow::Result;
use base64::Engine;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStyle {
    Bearer,
    XApiKey,
    Custom(Option<&'static str>), // Custom header name
}

pub struct OpenAiCompatibleProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub auth_style: AuthStyle,
    pub custom_header: Option<String>,
    pub client: Client,
    pub merge_system_into_user: bool,
    pub strip_thinking: bool,
}

impl OpenAiCompatibleProvider {
    pub fn new(name: &str, base_url: &str, api_key: &str) -> Self {
        Self {
            name: name.to_string(),
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            auth_style: AuthStyle::Bearer,
            custom_header: None,
            client: Client::builder().build().unwrap(),
            merge_system_into_user: false,
            strip_thinking: false,
        }
    }

    pub fn with_auth_style(mut self, style: AuthStyle) -> Self {
        self.auth_style = style;
        self
    }

    pub fn with_custom_header(mut self, header: &str) -> Self {
        self.custom_header = Some(header.to_string());
        self
    }

    pub fn with_system_merging(mut self, merge: bool) -> Self {
        self.merge_system_into_user = merge;
        self
    }

    pub fn with_thinking_stripping(mut self, strip: bool) -> Self {
        self.strip_thinking = strip;
        self
    }

    fn prepare_messages(&self, messages: &[ChatMessage]) -> Vec<Value> {
        let mut out = Vec::new();
        let mut system_prompt = String::new();

        for msg in messages {
            if self.merge_system_into_user && msg.role == "system" {
                if !system_prompt.is_empty() {
                    system_prompt.push_str("\n\n");
                }
                system_prompt.push_str(&msg.content);
                continue;
            }

            let mut msg_json = json!({
                "role": msg.role,
            });

            if !system_prompt.is_empty() && msg.role == "user" {
                let enriched_content = format!(
                    "[System Instructions]:\n{}\n\n{}",
                    system_prompt, msg.content
                );
                msg_json["content"] = json!(enriched_content);
                system_prompt.clear();
            } else if let Some(parts) = &msg.content_parts {
                let parts_json: Vec<Value> = parts
                    .iter()
                    .map(|p| match p {
                        ContentPart::Text(t) => json!({"type": "text", "text": t}),
                        ContentPart::ImageBase64 { data, media_type } => json!({
                            "type": "image_url",
                            "image_url": { "url": format!("data:{};base64,{}", media_type, data) }
                        }),
                        ContentPart::ImageUrl { url } => json!({
                            "type": "image_url",
                            "image_url": { "url": url }
                        }),
                        ContentPart::Media { mime_type, data } => {
                            let b64 = base64::engine::general_purpose::STANDARD.encode(data);
                            json!({
                                "type": "image_url",
                                "image_url": { "url": format!("data:{};base64,{}", mime_type, b64) }
                            })
                        }
                    })
                    .collect();
                msg_json["content"] = json!(parts_json);
            } else {
                msg_json["content"] = json!(msg.content);
            }

            if let Some(name) = &msg.name {
                msg_json["name"] = json!(name);
            }

            if let Some(tcs) = &msg.tool_calls {
                let tcs_json: Vec<Value> = tcs
                    .iter()
                    .map(|tc| {
                        json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.function.name,
                                "arguments": tc.function.arguments
                            }
                        })
                    })
                    .collect();
                msg_json["tool_calls"] = json!(tcs_json);
            }

            if let Some(tcid) = &msg.tool_call_id {
                msg_json["tool_call_id"] = json!(tcid);
            }

            out.push(msg_json);
        }

        out
    }

    fn build_request_body(&self, request: &ChatRequest, stream: bool) -> Value {
        let mut body = json!({
            "model": request.model,
            "messages": self.prepare_messages(request.messages),
            "temperature": request.temperature,
            "stream": stream,
        });

        let obj = body.as_object_mut().unwrap();

        if let Some(mt) = request.max_tokens {
            if request.reasoning_effort.is_some() && request.model.contains("o1") {
                obj.insert("max_completion_tokens".to_string(), json!(mt));
            } else {
                obj.insert("max_tokens".to_string(), json!(mt));
            }
        }

        if let Some(effort) = request.reasoning_effort {
            obj.insert("reasoning_effort".to_string(), json!(effort));
        }

        if let Some(tools) = request.tools
            && !tools.is_empty()
        {
            let tool_defs: Vec<_> = tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters
                        }
                    })
                })
                .collect();
            obj.insert("tools".to_string(), json!(tool_defs));
        }

        body
    }

    fn apply_auth(
        &self,
        builder: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        match self.auth_style {
            AuthStyle::Bearer => {
                builder.header("Authorization", format!("Bearer {}", self.api_key))
            }
            AuthStyle::XApiKey => builder.header("x-api-key", &self.api_key),
            AuthStyle::Custom(ref name) => {
                let header = name
                    .map(|s| s.to_string())
                    .or_else(|| self.custom_header.clone())
                    .unwrap_or_else(|| "Authorization".to_string());
                builder.header(header, &self.api_key)
            }
        }
    }
}

/// Extracts reasoning and content from a potentially mixed string containing <think> tags.
fn extract_reasoning(text: &str) -> (Option<String>, String) {
    if let Some(start_idx) = text.find("<think>") {
        if let Some(end_idx) = text.find("</think>") {
            let reasoning = text[start_idx + 7..end_idx].trim().to_string();
            let content = format!("{}{}", &text[..start_idx], &text[end_idx + 8..])
                .trim()
                .to_string();
            return (Some(reasoning), content);
        } else {
            // Unclosed think tag
            let reasoning = text[start_idx + 7..].trim().to_string();
            let content = text[..start_idx].trim().to_string();
            return (Some(reasoning), content);
        }
    }
    (None, text.to_string())
}

struct ThinkStripper {
    in_think: bool,
    buffer: String,
    strip: bool,
}

impl ThinkStripper {
    fn new(strip: bool) -> Self {
        Self {
            in_think: false,
            buffer: String::new(),
            strip,
        }
    }

    fn process(&mut self, delta: &str) -> (Option<String>, Option<String>) {
        if !self.strip {
            return (None, Some(delta.to_string()));
        }

        self.buffer.push_str(delta);
        let mut visible = String::new();
        let mut thought = String::new();

        loop {
            if self.in_think {
                if let Some(idx) = self.buffer.find("</think>") {
                    thought.push_str(&self.buffer[..idx]);
                    self.buffer = self.buffer[idx + 8..].to_string();
                    self.in_think = false;
                } else {
                    thought.push_str(&self.buffer);
                    self.buffer.clear();
                    break;
                }
            } else if let Some(idx) = self.buffer.find("<think>") {
                visible.push_str(&self.buffer[..idx]);
                self.buffer = self.buffer[idx + 7..].to_string();
                self.in_think = true;
            } else {
                // Check for partial <think tag at end of buffer
                if let Some(idx) = self.buffer.rfind('<') {
                    let potential = &self.buffer[idx..];
                    if "<think>".starts_with(potential) {
                        visible.push_str(&self.buffer[..idx]);
                        self.buffer = potential.to_string();
                        break;
                    }
                }
                visible.push_str(&self.buffer);
                self.buffer.clear();
                break;
            }
        }

        let v = if visible.is_empty() {
            None
        } else {
            Some(visible)
        };
        let t = if thought.is_empty() {
            None
        } else {
            Some(thought)
        };
        (t, v)
    }
}

impl Provider for OpenAiCompatibleProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let base = self.base_url.trim_end_matches('/');
        let endpoint = if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{}/chat/completions", base)
        };

        let body = self.build_request_body(request, false);

        let mut req = self
            .client
            .post(&endpoint)
            .timeout(Duration::from_secs(request.timeout_secs))
            .header("Content-Type", "application/json");

        req = self.apply_auth(req);

        let res = req.json(&body).send()?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().unwrap_or_default();
            anyhow::bail!("OpenAI-compatible API error {}: {}", status, text);
        }

        let parsed: Value = res.json()?;
        let choice = &parsed["choices"][0];
        let msg = &choice["message"];

        let mut raw_content = msg["content"].as_str().unwrap_or("").to_string();

        // Native reasoning fields
        let mut reasoning_content = msg["reasoning_content"]
            .as_str()
            .or_else(|| msg["reasoning"].as_str())
            .map(|s| s.to_string());

        // Extract reasoning from <think> tags if present in content
        if raw_content.contains("<think>") {
            let (extracted_think, cleaned_content) = extract_reasoning(&raw_content);
            if let Some(t) = extracted_think {
                if let Some(ref mut existing) = reasoning_content {
                    existing.push_str("\n\n");
                    existing.push_str(&t);
                } else {
                    reasoning_content = Some(t);
                }
            }
            raw_content = cleaned_content;
        }

        let mut tool_calls = Vec::new();
        if let Some(tcs) = msg["tool_calls"].as_array() {
            for tc in tcs {
                tool_calls.push(ToolCall {
                    id: tc["id"].as_str().unwrap_or_default().to_string(),
                    kind: tc["type"].as_str().unwrap_or("function").to_string(),
                    function: FunctionCall {
                        name: tc["function"]["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        arguments: tc["function"]["arguments"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        thought_signature: None,
                    },
                });
            }
        }

        let usage_val = &parsed["usage"];
        let usage = TokenUsage {
            prompt_tokens: usage_val["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            completion_tokens: usage_val["completion_tokens"].as_u64().unwrap_or(0) as u32,
            total_tokens: usage_val["total_tokens"].as_u64().unwrap_or(0) as u32,
        };

        Ok(ChatResponse {
            content: if raw_content.is_empty() {
                None
            } else {
                Some(raw_content)
            },
            tool_calls,
            usage,
            model: request.model.to_string(),
            reasoning_content,
            thought_signature: None,
        })
    }

    fn supports_native_tools(&self) -> bool {
        true
    }

    fn get_name(&self) -> &str {
        &self.name
    }

    fn chat_stream(
        &self,
        request: &ChatRequest,
        mut callback: StreamCallback,
    ) -> Result<ChatResponse> {
        use crate::providers::sse::SseReader;

        let base = self.base_url.trim_end_matches('/');
        let endpoint = if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{}/chat/completions", base)
        };

        let body = self.build_request_body(request, true);

        let mut req = self
            .client
            .post(&endpoint)
            .timeout(Duration::from_secs(request.timeout_secs))
            .header("Content-Type", "application/json");

        req = self.apply_auth(req);

        let res = req.json(&body).send()?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().unwrap_or_default();
            anyhow::bail!("OpenAI-compatible streaming API error {}: {}", status, text);
        }

        let mut sse_reader = SseReader::new(res);
        let mut full_content = String::new();
        let mut full_reasoning = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut tc_arg_bufs: Vec<String> = Vec::new();

        let mut stripper = ThinkStripper::new(true); // Default to true for better UX

        while let Some(data) = sse_reader.next_data() {
            if data == "[DONE]" {
                break;
            }

            if let Ok(parsed) = serde_json::from_str::<Value>(&data) {
                let delta = &parsed["choices"][0]["delta"];

                // Native reasoning delta
                if let Some(reasoning) = delta
                    .get("reasoning_content")
                    .or_else(|| delta.get("reasoning"))
                    .and_then(|v| v.as_str())
                {
                    full_reasoning.push_str(reasoning);
                    // For now, we don't stream reasoning content deltas separately in our StreamChunk enum,
                    // but we could extend it if needed.
                }

                // Content delta with think-stripping
                if let Some(text) = delta.get("content").and_then(|v| v.as_str()) {
                    let (thought, visible) = stripper.process(text);
                    if let Some(t) = thought {
                        full_reasoning.push_str(&t);
                    }
                    if let Some(v) = visible {
                        full_content.push_str(&v);
                        callback(StreamChunk::Delta(v));
                    }
                }

                // Tool calls
                if let Some(tcs) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc_delta in tcs {
                        let idx = tc_delta["index"].as_u64().unwrap_or(0) as usize;
                        while tool_calls.len() <= idx {
                            tool_calls.push(ToolCall {
                                id: String::new(),
                                kind: "function".to_string(),
                                function: FunctionCall {
                                    name: String::new(),
                                    arguments: String::new(),
                                    thought_signature: None,
                                },
                            });
                            tc_arg_bufs.push(String::new());
                        }

                        if let Some(id) = tc_delta["id"].as_str() {
                            tool_calls[idx].id = id.to_string();
                        }
                        if let Some(name) = tc_delta["function"]["name"].as_str() {
                            tool_calls[idx].function.name = name.to_string();
                        }
                        let args = tc_delta["function"]["arguments"].as_str().unwrap_or("");
                        tc_arg_bufs[idx].push_str(args);

                        callback(StreamChunk::ToolCallDelta {
                            index: idx,
                            id: tc_delta["id"].as_str().map(|s| s.to_string()),
                            name: tc_delta["function"]["name"].as_str().map(|s| s.to_string()),
                            arguments_delta: args.to_string(),
                        });
                    }
                }
            }
        }

        for (i, buf) in tc_arg_bufs.into_iter().enumerate() {
            if i < tool_calls.len() {
                tool_calls[i].function.arguments = buf;
            }
        }

        let usage = TokenUsage {
            prompt_tokens: 0, // Estimate or parse from final chunk if provider supports it
            completion_tokens: (full_content.len() as u32).div_ceil(4),
            total_tokens: (full_content.len() as u32).div_ceil(4),
        };

        callback(StreamChunk::Done(usage.clone()));

        Ok(ChatResponse {
            content: if full_content.is_empty() {
                None
            } else {
                Some(full_content)
            },
            tool_calls,
            usage,
            model: request.model.to_string(),
            reasoning_content: if full_reasoning.is_empty() {
                None
            } else {
                Some(full_reasoning)
            },
            thought_signature: None,
        })
    }
}
