use crate::providers::{
    ChatRequest, ChatResponse, FunctionCall, Provider, StreamCallback, TokenUsage, ToolCall,
};
use anyhow::Result;
use reqwest::blocking::Client;
use serde_json::json;
use std::time::Duration;

/// OllamaProvider — uses Ollama's OpenAI-compatible API.
/// Default base_url is http://localhost:11434/v1
pub struct OllamaProvider {
    pub base_url: String,
    pub client: Client,
}

impl OllamaProvider {
    pub fn new(base_url: Option<&str>) -> Self {
        let url = base_url
            .unwrap_or("http://localhost:11434/v1")
            .trim_end_matches('/')
            .to_string();
        Self {
            base_url: url,
            client: Client::builder().build().unwrap(),
        }
    }

    /// Helper to check if Ollama is actually reachable
    pub fn is_available(&self) -> bool {
        // Try to hit the tags endpoint or just the base
        let tags_url = self.base_url.replace("/v1", "/api/tags");
        self.client.get(&tags_url).timeout(Duration::from_secs(2)).send().is_ok()
    }
}

impl Provider for OllamaProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let endpoint = format!("{}/chat/completions", self.base_url);

        let mut body = json!({
            "model": request.model,
            "messages": request.messages,
            "temperature": request.temperature,
        });

        if let Some(obj) = body.as_object_mut() {
            if let Some(mt) = request.max_tokens {
                obj.insert("max_tokens".to_string(), json!(mt));
            }

            if let Some(tools) = request.tools
                && !tools.is_empty() {
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

        let res = self
            .client
            .post(&endpoint)
            .timeout(Duration::from_secs(request.timeout_secs))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().unwrap_or_default();
            anyhow::bail!("Ollama API error {}: {}", status, text);
        }

        let parsed: serde_json::Value = res.json()?;

        let choice = &parsed["choices"][0];
        let msg = &choice["message"];

        let content = msg["content"].as_str().map(|s| s.to_string());

        let mut tool_calls = Vec::new();
        if let Some(tcs) = msg["tool_calls"].as_array() {
            for tc in tcs {
                let id = tc["id"].as_str().unwrap_or_default().to_string();
                let kind = tc["type"].as_str().unwrap_or("function").to_string();
                let f = &tc["function"];
                let name = f["name"].as_str().unwrap_or_default().to_string();
                let arguments = f["arguments"].as_str().unwrap_or_default().to_string();

                tool_calls.push(ToolCall {
                    id,
                    kind,
                    function: FunctionCall { name, arguments },
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
            content,
            tool_calls,
            usage,
            model: request.model.to_string(),
            reasoning_content: None,
        })
    }

    fn supports_native_tools(&self) -> bool {
        // Ollama supports tools in newer versions (0.3.0+)
        true
    }

    fn get_name(&self) -> &str {
        "ollama"
    }

    fn chat_stream(
        &self,
        request: &ChatRequest,
        mut callback: StreamCallback,
    ) -> Result<ChatResponse> {
        use crate::providers::StreamChunk;
        use crate::providers::sse::SseReader;

        let endpoint = format!("{}/chat/completions", self.base_url);

        let mut body = json!({
            "model": request.model,
            "messages": request.messages,
            "temperature": request.temperature,
            "stream": true,
        });

        if let Some(obj) = body.as_object_mut()
            && let Some(mt) = request.max_tokens {
                obj.insert("max_tokens".to_string(), json!(mt));
            }

        let res = self
            .client
            .post(&endpoint)
            .timeout(Duration::from_secs(request.timeout_secs))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().unwrap_or_default();
            anyhow::bail!("Ollama streaming API error {}: {}", status, text);
        }

        let mut sse_reader = SseReader::new(res);
        let mut full_content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut tc_arg_bufs: Vec<String> = Vec::new();

        while let Some(data) = sse_reader.next_data() {
            if data == "[DONE]" {
                break;
            }

            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data)
                && let Some(delta) = parsed["choices"][0]["delta"].as_object() {
                    if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                        full_content.push_str(text);
                        callback(StreamChunk::Delta(text.to_string()));
                    }

                    if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc_delta in tcs {
                            let idx = tc_delta["index"].as_u64().unwrap_or(0) as usize;
                            let id = tc_delta["id"].as_str().map(|s| s.to_string());
                            let name = tc_delta["function"]["name"].as_str().map(|s| s.to_string());
                            let args_delta = tc_delta["function"]["arguments"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();

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

        for (i, buf) in tc_arg_bufs.into_iter().enumerate() {
            if i < tool_calls.len() {
                tool_calls[i].function.arguments = buf;
            }
        }

        let prompt_tokens: u32 = request.messages.iter().map(|m| m.content.len() as u32 / 4).sum();
        let completion_tokens = (full_content.len() as u32).div_ceil(4);
        let usage = TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        };

        callback(StreamChunk::Done(usage.clone()));

        Ok(ChatResponse {
            content: if full_content.is_empty() { None } else { Some(full_content) },
            tool_calls,
            usage,
            model: request.model.to_string(),
            reasoning_content: None,
        })
    }
}
