use crate::providers::{
    ChatRequest, ChatResponse, FunctionCall, Provider, StreamCallback, TokenUsage, ToolCall,
};
use anyhow::Result;
use reqwest::blocking::Client;
use serde_json::json;
use std::time::Duration;

pub struct OpenAiCompatibleProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub client: Client,
}

impl OpenAiCompatibleProvider {
    pub fn new(name: &str, base_url: &str, api_key: &str) -> Self {
        Self {
            name: name.to_string(),
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            client: Client::builder().build().unwrap(),
        }
    }
}

// Internal serialization struct to match OpenAI API strictness if needed,
// but usually we can serialize ChatMessage directly if fields match.
// However, OpenAI is picky about null vs absent fields sometimes.
// Using ChatMessage from mod.rs which has skip_serializing_if should be fine.

impl Provider for OpenAiCompatibleProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let endpoint = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let mut body = json!({
            "model": request.model,
            "messages": request.messages,
            "temperature": request.temperature,
        });

        if let Some(obj) = body.as_object_mut() {
            // Handle max_tokens vs max_completion_tokens based on model (o1 etc.)
            // For now, simpler: use max_tokens if not reasoning, else...
            // Actually, newer models prefer max_completion_tokens.
            if let Some(mt) = request.max_tokens {
                if request.reasoning_effort.is_some() {
                    obj.insert("max_completion_tokens".to_string(), json!(mt));
                } else {
                    obj.insert("max_tokens".to_string(), json!(mt));
                }
            }

            if let Some(effort) = request.reasoning_effort {
                obj.insert("reasoning_effort".to_string(), json!(effort));
            }

            if let Some(tools) = request.tools {
                if !tools.is_empty() {
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
            }
        }

        let res = self
            .client
            .post(&endpoint)
            .timeout(Duration::from_secs(request.timeout_secs))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().unwrap_or_default();
            anyhow::bail!("OpenAI API error {}: {}", status, text);
        }

        let parsed: serde_json::Value = res.json()?;

        let choice = parsed["choices"][0].clone();
        let msg = choice["message"].clone();

        let content = msg["content"].as_str().map(|s| s.to_string());

        // Handle reasoning_content (DeepSeek style or standard?)
        // Standard OpenAI doesn't output reasoning_content field yet, but some compatible providers might.
        // For o1, it's hidden. DeepSeek R1 returns it.
        let reasoning_content = msg["reasoning_content"].as_str().map(|s| s.to_string());

        let mut tool_calls = Vec::new();
        if let Some(tcs) = msg["tool_calls"].as_array() {
            for tc in tcs {
                let id = tc["id"].as_str().unwrap_or_default().to_string();
                let kind = tc["type"].as_str().unwrap_or("function").to_string();
                let f = tc["function"].clone();
                let name = f["name"].as_str().unwrap_or_default().to_string();
                let arguments = f["arguments"].as_str().unwrap_or_default().to_string();

                tool_calls.push(ToolCall {
                    id,
                    kind,
                    function: FunctionCall { name, arguments },
                });
            }
        }

        let usage_val = parsed["usage"].clone();
        let usage = TokenUsage {
            prompt_tokens: usage_val["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            completion_tokens: usage_val["completion_tokens"].as_u64().unwrap_or(0) as u32,
            total_tokens: usage_val["total_tokens"].as_u64().unwrap_or(0) as u32,
        };

        Ok(ChatResponse {
            content,
            tool_calls,
            usage,
            model: request.model.to_string(),
            reasoning_content,
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
    ) -> anyhow::Result<ChatResponse> {
        use crate::providers::StreamChunk;
        use crate::providers::sse::SseReader;

        let endpoint = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": request.messages,
            "temperature": request.temperature,
            "stream": true,
        });

        if let Some(obj) = body.as_object_mut() {
            if let Some(mt) = request.max_tokens {
                if request.reasoning_effort.is_some() {
                    obj.insert("max_completion_tokens".to_string(), serde_json::json!(mt));
                } else {
                    obj.insert("max_tokens".to_string(), serde_json::json!(mt));
                }
            }
            if let Some(effort) = request.reasoning_effort {
                obj.insert("reasoning_effort".to_string(), serde_json::json!(effort));
            }
            if let Some(tools) = request.tools {
                if !tools.is_empty() {
                    let tool_defs: Vec<_> = tools
                        .iter()
                        .map(|t| {
                            serde_json::json!({
                                "type": "function",
                                "function": {
                                    "name": t.name,
                                    "description": t.description,
                                    "parameters": t.parameters
                                }
                            })
                        })
                        .collect();
                    obj.insert("tools".to_string(), serde_json::json!(tool_defs));
                }
            }
        }

        let res = self
            .client
            .post(&endpoint)
            .timeout(std::time::Duration::from_secs(request.timeout_secs))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().unwrap_or_default();
            anyhow::bail!("OpenAI streaming API error {}: {}", status, text);
        }

        let mut sse_reader = SseReader::new(res);
        let mut full_content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        // Track partial tool call argument accumulation by index
        let mut tc_arg_bufs: Vec<String> = Vec::new();

        while let Some(data) = sse_reader.next_data() {
            if data == "[DONE]" {
                break;
            }

            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(delta) = parsed["choices"][0]["delta"].as_object() {
                    // Text content delta
                    if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                        full_content.push_str(text);
                        callback(StreamChunk::Delta(text.to_string()));
                    }

                    // Tool call deltas
                    if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc_delta in tcs {
                            let idx = tc_delta["index"].as_u64().unwrap_or(0) as usize;
                            let id = tc_delta["id"].as_str().map(|s| s.to_string());
                            let name = tc_delta["function"]["name"].as_str().map(|s| s.to_string());
                            let args_delta = tc_delta["function"]["arguments"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();

                            // Grow tool_calls and arg buffers if needed
                            while tool_calls.len() <= idx {
                                tool_calls.push(ToolCall {
                                    id: String::new(),
                                    kind: "function".to_string(),
                                    function: FunctionCall {
                                        name: String::new(),
                                        arguments: String::new(),
                                    },
                                });
                                tc_arg_bufs.push(String::new());
                            }

                            if let Some(ref id_val) = id {
                                tool_calls[idx].id = id_val.clone();
                            }
                            if let Some(ref name_val) = name {
                                tool_calls[idx].function.name = name_val.clone();
                            }
                            tc_arg_bufs[idx].push_str(&args_delta);

                            callback(StreamChunk::ToolCallDelta {
                                index: idx,
                                id,
                                name,
                                arguments_delta: args_delta,
                            });
                        }
                    }
                }
            }
        }

        // Finalize tool call arguments
        for (i, buf) in tc_arg_bufs.into_iter().enumerate() {
            if i < tool_calls.len() {
                tool_calls[i].function.arguments = buf;
            }
        }

        let prompt_tokens: u32 = request
            .messages
            .iter()
            .map(|m| m.content.len() as u32 / 4)
            .sum();
        let completion_tokens = (full_content.len() as u32 + 3) / 4;
        let usage = TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
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
            reasoning_content: None,
        })
    }
}
