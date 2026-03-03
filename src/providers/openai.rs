use crate::providers::{
    ChatMessage, ChatRequest, ChatResponse, FunctionCall, Provider, TokenUsage, ToolCall,
};
use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
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
                    let tool_defs: Vec<_> = tools.iter().map(|t| {
                        json!({
                            "type": "function",
                            "function": {
                                "name": t.name,
                                "description": t.description,
                                "parameters": t.parameters
                            }
                        })
                    }).collect();
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
                    function: FunctionCall {
                        name,
                        arguments,
                    },
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
}
