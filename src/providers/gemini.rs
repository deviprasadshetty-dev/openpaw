use crate::providers::{
    ChatRequest, ChatResponse, ContentPart, Provider, StreamCallback, TokenUsage,
};
use base64::Engine;
use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde_json::json;
use std::time::Duration;

const BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 8192;

/// Authentication method for Gemini.
#[derive(Debug, Clone)]
pub enum GeminiAuth {
    /// Explicit API key from config: sent as `?key=` query parameter.
    ExplicitKey(String),
    /// API key from `GEMINI_API_KEY` env var.
    EnvGeminiKey(String),
    /// API key from `GOOGLE_API_KEY` env var.
    EnvGoogleKey(String),
}


impl GeminiAuth {
    pub fn is_api_key(&self) -> bool {
        true
    }

    pub fn credential(&self) -> &str {
        match self {
            GeminiAuth::ExplicitKey(v) => v,
            GeminiAuth::EnvGeminiKey(v) => v,
            GeminiAuth::EnvGoogleKey(v) => v,
        }
    }

    pub fn source(&self) -> &str {
        match self {
            GeminiAuth::ExplicitKey(_) => "config",
            GeminiAuth::EnvGeminiKey(_) => "GEMINI_API_KEY env var",
            GeminiAuth::EnvGoogleKey(_) => "GOOGLE_API_KEY env var",
        }
    }
}


/// Google Gemini provider using API keys.
pub struct GeminiProvider {
    auth: Option<GeminiAuth>,
    client: Client,
}

impl GeminiProvider {
    pub fn new(api_key: Option<&str>) -> Self {
        let mut auth: Option<GeminiAuth> = None;

        // 1. Explicit key
        if let Some(key) = api_key {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                auth = Some(GeminiAuth::ExplicitKey(trimmed.to_string()));
            }
        }

        // 2. Environment API keys
        if auth.is_none() && let Ok(value) = std::env::var("GEMINI_API_KEY") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                auth = Some(GeminiAuth::EnvGeminiKey(trimmed.to_string()));
            }
        }

        if auth.is_none() && let Ok(value) = std::env::var("GOOGLE_API_KEY") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                auth = Some(GeminiAuth::EnvGoogleKey(trimmed.to_string()));
            }
        }

        Self {
            auth,
            client: Client::builder().build().unwrap(),
        }
    }

    /// Get authentication source description for diagnostics.
    pub fn auth_source(&self) -> &str {
        match &self.auth {
            Some(auth) => auth.source(),
            None => "none",
        }
    }

    fn build_request_target(
        &self,
        request: &ChatRequest,
        auth: &GeminiAuth,
        streaming: bool,
    ) -> Result<(String, String)> {
        let model_name = if request.model.starts_with("models/") {
            request.model.to_string()
        } else {
            format!("models/{}", request.model)
        };

        let action = if streaming {
            "streamGenerateContent?alt=sse"
        } else {
            "generateContent"
        };

        let body = self.build_request_body(request)?;
        let separator = if action.contains('?') { "&" } else { "?" };
        let url = format!(
            "{}/{}:{}{}key={}",

            BASE_URL,
            model_name,
            action,
            separator,
            auth.credential()
        );
        Ok((url, body))

    }

    /// Build a Gemini generateContent request body.
    fn build_request_body(&self, request: &ChatRequest) -> Result<String> {
        let mut contents = Vec::new();
        let mut system_prompt: Option<String> = None;

        // Group consecutive messages by role for Gemini's strict alternation
        let mut grouped_messages: Vec<(String, Vec<serde_json::Value>)> = Vec::new();

        for msg in request.messages {
            if msg.role == "system" {
                system_prompt = Some(msg.content.clone());
                continue;
            }

            // Map roles: Gemini uses "user" and "model"
            let role = match msg.role.as_str() {
                "user" | "tool" => "user",
                "assistant" => "model",
                _ => "user",
            };

            let mut parts = Vec::new();

            // Handle native tool results (functionResponse)
            if msg.role == "tool" {
                if msg.tool_call_id.is_some() {
                    let part = json!({
                        "functionResponse": {
                            "name": msg.name.as_deref().unwrap_or("unknown"),
                            "response": {
                                "content": msg.content
                            }
                        }
                    });

                    parts.push(part);
                } else {
                    // Fallback for non-native tool results
                    parts.push(json!({"text": format!("[Tool Result]:\n{}", msg.content)}));
                }
            } else if let Some(tool_calls) = &msg.tool_calls {
                // Handle assistant native tool calls (functionCall)
                if !msg.content.is_empty() {
                    let text_part = json!({"text": msg.content});
                    parts.push(text_part);
                }
                for (idx, tc) in tool_calls.iter().enumerate() {
                    let fc_obj = json!({
                        "name": tc.function.name,
                        "args": serde_json::from_str::<serde_json::Value>(&tc.function.arguments).unwrap_or(json!({}))
                    });

                    let mut part = json!({
                        "functionCall": fc_obj
                    });

                    // Gemini 3.x models require thought_signature for all functionCall parts.
                    // The signature should be on the first functionCall in a turn (idx == 0).
                    // If we have a real signature from the model, use it; otherwise use a dummy
                    // signature for compatibility (required for Gemini 3.x strict validation).
                    let sig = if idx == 0 {
                        tc.function.thought_signature.as_deref().or_else(|| {
                            // Dummy signature for function calls not generated by this API turn
                            // This prevents 400 errors on Gemini 3.x models
                            Some("skip_thought_signature_validator")
                        })
                    } else {
                        tc.function.thought_signature.as_deref()
                    };

                    if let Some(signature) = sig {
                        part["thought_signature"] = json!(signature);
                    }

                    parts.push(part);
                }
            } else if let Some(content_parts) = msg.content_parts.as_ref() {
                // Multimodal support
                for part in content_parts {
                    match part {
                        ContentPart::Text(text) => {
                            let p = json!({"text": text});
                            parts.push(p);
                        }
                        ContentPart::ImageBase64 { data, media_type } => {
                            let p = json!({
                                "inlineData": {
                                    "mimeType": media_type,
                                    "data": data
                                }
                            });
                            parts.push(p);
                        }
                        ContentPart::Media { mime_type, data } => {
                            let p = json!({
                                "inlineData": {
                                    "mimeType": mime_type,
                                    "data": base64::engine::general_purpose::STANDARD.encode(data)
                                }
                            });
                            parts.push(p);
                        }
                        ContentPart::ImageUrl { url } => {
                            // Gemini doesn't support direct image URLs in the same way as OpenAI
                            // Fallback to text placeholder
                            parts.push(json!({"text": format!("[Image: {}]", url)}));
                        }

                    }
                }
            } else {
                let p = json!({"text": msg.content});
                parts.push(p);
            }

            if let Some(last) = grouped_messages.last_mut() {
                if last.0 == role {
                    last.1.extend(parts);
                } else {
                    grouped_messages.push((role.to_string(), parts));
                }
            } else {
                grouped_messages.push((role.to_string(), parts));
            }
        }

        for (role, parts) in grouped_messages {
            contents.push(json!({
                "role": role,
                "parts": parts
            }));
        }

        let mut generation_config = json!({
            "temperature": request.temperature,
            "maxOutputTokens": request.max_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
        });

        // Handle reasoning/thinking models
        if let Some(effort) = request.reasoning_effort
            && effort != "none"
        {
            let model_lower = request.model.to_lowercase();

            if model_lower.contains("gemini-3.") || model_lower.contains("flash-thinking") {
                // Gemini 3.x and specific models use thinkingLevel: "low", "medium", "high"
                generation_config["thinkingConfig"] = json!({
                    "thinkingLevel": effort
                });
            } else {
                // Gemini 2.x and others use thinkingBudget: integer
                let budget = match effort {
                    "low" => 4096,
                    "medium" => 12288,
                    "high" => 24576,
                    _ => 12288,
                };
                generation_config["thinkingConfig"] = json!({
                    "includeThoughts": true,
                    "thinkingBudget": budget
                });
            }
        }

        let mut body = json!({
            "contents": contents,
            "generationConfig": generation_config
        });

        if let Some(sys) = system_prompt {
            body["system_instruction"] = json!({
                "parts": [{"text": sys}]
            });
        }

        // Don't send tools to Gemini API - it requires strict function calling
        // which causes 400 errors. We use XML-style tool calls instead.
        // See: https://ai.google.dev/api/generate-content#functionresponse
        // NOTE: Tools are described in the system prompt instead.

        Ok(body.to_string())
    }

    fn normalize_token_usage(usage: &mut TokenUsage) {
        if usage.total_tokens == 0 && (usage.prompt_tokens > 0 || usage.completion_tokens > 0) {
            usage.total_tokens = usage.prompt_tokens + usage.completion_tokens;
        }
        if usage.completion_tokens == 0 && usage.total_tokens > usage.prompt_tokens {
            usage.completion_tokens = usage.total_tokens - usage.prompt_tokens;
        }
    }

    fn parse_usage_metadata(v: &serde_json::Value) -> Option<TokenUsage> {
        let obj = v.as_object()?;
        let mut usage = TokenUsage::default();
        let mut found = false;

        if let Some(count) = obj.get("promptTokenCount").and_then(|c| c.as_u64()) {
            usage.prompt_tokens = count as u32;
            found = true;
        }
        if let Some(count) = obj.get("candidatesTokenCount").and_then(|c| c.as_u64()) {
            usage.completion_tokens = count as u32;
            found = true;
        }
        if let Some(count) = obj.get("totalTokenCount").and_then(|c| c.as_u64()) {
            usage.total_tokens = count as u32;
            found = true;
        }

        if !found {
            return None;
        }
        Self::normalize_token_usage(&mut usage);
        Some(usage)
    }

    /// Parse text content and tool calls from a Gemini generateContent response.
    fn parse_response(&self, body: &str) -> Result<ChatResponse> {
        let parsed: serde_json::Value =
            serde_json::from_str(body).context("Failed to parse Gemini response")?;

        if let Some(error) = parsed.get("error") {
            let msg = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown Gemini API error");
            let code = error.get("code").and_then(|c| c.as_u64()).unwrap_or(0);
            
            // Log for diagnostics
            tracing::error!("Gemini API error (code {}): {}", code, msg);

            // Classify error for retry logic
            let kind = crate::providers::error_classify::classify(code as u16, body);
            if kind == crate::providers::error_classify::ApiErrorKind::RateLimit {
                anyhow::bail!("Gemini API rate limited: {}", msg);
            }
            if kind == crate::providers::error_classify::ApiErrorKind::Quota {
                anyhow::bail!("Gemini API quota exceeded: {}", msg);
            }

            anyhow::bail!("Gemini API error ({}): {}", code, msg);
        }


        let response_root = parsed.get("response").unwrap_or(&parsed);

        if let Some(feedback) = response_root.get("promptFeedback")
            && let Some(block_reason) = feedback.get("blockReason")
        {
            let reason = block_reason.as_str().unwrap_or("unknown");
            anyhow::bail!("Gemini blocked the prompt: {}", reason);
        }

        let mut content = String::new();
        let mut reasoning_content = String::new();
        let mut tool_calls = Vec::new();
        let mut turn_thought_signature: Option<String> = None;

        if let Some(candidates) = response_root.get("candidates")
            && let Some(candidate) = candidates.get(0)
        {
            if let Some(finish_reason) = candidate.get("finishReason") {
                let reason = finish_reason.as_str().unwrap_or("unknown");
                if reason != "STOP" && reason != "MAX_TOKENS" {
                    tracing::warn!("Gemini finish reason: {}", reason);
                }
            }

            if let Some(cand_content) = candidate.get("content")
                && let Some(parts) = cand_content.get("parts")
                && let Some(parts_array) = parts.as_array()
            {
                // First pass: find ANY thought_signature in the whole turn
                for part in parts_array {
                    if let Some(sig) = part.get("thought_signature").and_then(|v| v.as_str()) {
                        turn_thought_signature = Some(sig.to_string());
                        break;
                    }
                }

                for part in parts_array {
                    // Support for reasoning (thought) in newer Gemini models.
                    // Gemini uses a boolean "thought" flag in the Part, while the content is in "text".
                    let is_thought = part.get("thought").and_then(|t| t.as_bool()).unwrap_or(false);
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        if is_thought {
                            reasoning_content.push_str(text);
                        } else {
                            content.push_str(text);
                        }
                    }
                    if let Some(fc) = part.get("functionCall")
                        && let Some(name) = fc.get("name").and_then(|n| n.as_str())
                    {
                        let args = fc.get("args").cloned().unwrap_or(json!({}));
                        let call_id = format!("call_{}", uuid::Uuid::new_v4().simple());

                        // thought_signature is a sibling to functionCall in the Part
                        // If missing on this part, use the turn-level one we found
                        let part_signature = part
                            .get("thought_signature")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let final_signature =
                            part_signature.or_else(|| turn_thought_signature.clone());

                        tool_calls.push(crate::providers::ToolCall {
                            id: call_id,
                            kind: "function".to_string(),
                            function: crate::providers::FunctionCall {
                                name: name.to_string(),
                                arguments: args.to_string(),
                                thought_signature: final_signature,
                            },
                        });
                    }
                }
            }
        }

        let mut usage = response_root
            .get("usageMetadata")
            .and_then(Self::parse_usage_metadata)
            .unwrap_or_default();

        if usage.total_tokens == 0 {
            usage.prompt_tokens = 0; // Estimation fallback
            usage.completion_tokens = (content.len() as u32).div_ceil(4);
            usage.total_tokens = usage.completion_tokens;
        }

        Ok(ChatResponse {
            content: if content.is_empty() && tool_calls.is_empty() {
                None
            } else {
                Some(content)
            },
            tool_calls,
            usage,
            model: "gemini".to_string(),
            reasoning_content: if reasoning_content.is_empty() {
                None
            } else {
                Some(reasoning_content)
            },
            thought_signature: turn_thought_signature,
        })
    }
}

impl Provider for GeminiProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let auth = self
            .auth
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No Gemini credentials configured."))?;

        let (url, body) = self.build_request_target(request, auth, false)?;

        let res = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(request.timeout_secs))
            .header("Content-Type", "application/json")
            .body(body)
            .send()?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().unwrap_or_default();
            anyhow::bail!("Gemini API error {}: {}", status, text);
        }

        let resp_text = res.text()?;
        let mut response = self.parse_response(&resp_text)?;
        response.model = request.model.to_string();
        Ok(response)
    }

    fn supports_native_tools(&self) -> bool {
        // Gemini's API requires strict alternation: functionResponse must come IMMEDIATELY
        // after functionCall with no intervening messages. Our agent loop may insert
        // nudges or other messages, violating this requirement and causing 400 errors.
        // Use XML-style tool calls (more robust) instead of native function calling.
        // See: https://ai.google.dev/api/generate-content#functionresponse
        false
    }

    fn get_name(&self) -> &str {
        "gemini"
    }

    fn chat_stream(
        &self,
        request: &ChatRequest,
        mut callback: StreamCallback,
    ) -> Result<ChatResponse> {
        use crate::providers::StreamChunk;
        use crate::providers::sse::SseReader;

        let auth = self
            .auth
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No Gemini credentials configured."))?;
        let (url, body) = self.build_request_target(request, &auth, true)?;

        let res = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(request.timeout_secs))
            .header("Content-Type", "application/json")
            .body(body)
            .send()?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().unwrap_or_default();
            anyhow::bail!("Gemini streaming API error {}: {}", status, text);
        }

        let mut sse_reader = SseReader::new(res);
        let mut full_content = String::new();
        let mut tool_calls = Vec::new();
        let mut turn_thought_signature: Option<String> = None;
        let mut stream_usage = TokenUsage::default();
        let mut reasoning_content = String::new();

        while let Some(data) = sse_reader.next_data() {
            if data == "[DONE]" {
                break;
            }

            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data) {
                let response_root = parsed.get("response").unwrap_or(&parsed);
                if let Some(candidates) = response_root.get("candidates")
                    && let Some(candidate) = candidates.get(0)
                    && let Some(cand_content) = candidate.get("content")
                    && let Some(parts) = cand_content.get("parts")
                    && let Some(parts_array) = parts.as_array()
                {
                    // Capture any thought_signature and usage metadata seen in the stream
                    if let Some(usage_val) = response_root.get("usageMetadata") {
                        if let Some(parsed_usage) = Self::parse_usage_metadata(usage_val) {
                            stream_usage = parsed_usage;
                        }
                    }

                    for part in parts_array {
                        if let Some(sig) = part.get("thought_signature").and_then(|ts| ts.as_str())
                        {
                            turn_thought_signature = Some(sig.to_string());
                        }
                    }

                    for part in parts_array {
                        let is_thought = part.get("thought").and_then(|t| t.as_bool()).unwrap_or(false);
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            if is_thought {
                                reasoning_content.push_str(text);
                            } else {
                                full_content.push_str(text);
                                callback(StreamChunk::Delta(text.to_string()));
                            }
                        }
                        if let Some(fc) = part.get("functionCall")
                            && let Some(name) = fc.get("name").and_then(|n| n.as_str())
                        {
                            let args = fc.get("args").cloned().unwrap_or(json!({}));
                            let call_id = format!("call_{}", uuid::Uuid::new_v4().simple());

                            let part_signature = part
                                .get("thought_signature")
                                .and_then(|ts| ts.as_str())
                                .map(|s| s.to_string());
                            let final_signature =
                                part_signature.or_else(|| turn_thought_signature.clone());

                            let tc = crate::providers::ToolCall {
                                id: call_id,
                                kind: "function".to_string(),
                                function: crate::providers::FunctionCall {
                                    name: name.to_string(),
                                    arguments: args.to_string(),
                                    thought_signature: final_signature,
                                },
                            };
                            tool_calls.push(tc);
                        }
                    }
                }
            }
        }

        let mut usage = stream_usage;
        if usage.total_tokens == 0 {
            usage.prompt_tokens = 0;
            usage.completion_tokens = (full_content.len() as u32).div_ceil(4);
            usage.total_tokens = usage.completion_tokens;
        }

        callback(StreamChunk::Done(usage.clone()));

        Ok(ChatResponse {
            content: if full_content.is_empty() && tool_calls.is_empty() {
                None
            } else {
                Some(full_content)
            },
            tool_calls,
            usage,
            model: request.model.to_string(),
            reasoning_content: if reasoning_content.is_empty() {
                None
            } else {
                Some(reasoning_content)
            },
            thought_signature: turn_thought_signature,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_auth_source() {
        assert_eq!(GeminiAuth::ExplicitKey("k".to_string()).source(), "config");
        assert_eq!(
            GeminiAuth::EnvGeminiKey("k".to_string()).source(),
            "GEMINI_API_KEY env var"
        );
        assert_eq!(
            GeminiAuth::EnvGoogleKey("k".to_string()).source(),
            "GOOGLE_API_KEY env var"
        );
    }




    #[test]
    fn test_parse_response_success() {
        let provider = GeminiProvider::new(None);
        let body = r#"{
            "candidates": [
                {
                    "content": {
                        "parts": [{"text": "hello"}]
                    },
                    "finishReason": "STOP"
                }
            ]
        }"#;

        let parsed = provider.parse_response(body).unwrap();
        assert_eq!(parsed.content, Some("hello".to_string()));
    }

    #[test]
    fn test_parse_response_with_reasoning() {
        let provider = GeminiProvider::new(None);
        let body = r#"{
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {"thought": true, "text": "I should say hello"},
                            {"text": "Hello!"}
                        ]
                    },
                    "finishReason": "STOP"
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 20,
                "totalTokenCount": 30
            }
        }"#;

        let parsed = provider.parse_response(body).unwrap();
        assert_eq!(parsed.content, Some("Hello!".to_string()));
        assert_eq!(
            parsed.reasoning_content,
            Some("I should say hello".to_string())
        );
        assert_eq!(parsed.usage.total_tokens, 30);
    }

    #[test]
    fn test_thinking_config_mapping() {
        let mut messages = Vec::new();
        messages.push(crate::providers::ChatMessage::user("hello"));

        // Test Gemini 3.x thinkingLevel
        let req_g3 = ChatRequest {
            messages: &messages,
            model: "gemini-3.1-flash",
            temperature: 0.7,
            max_tokens: None,
            tools: None,
            timeout_secs: 30,
            reasoning_effort: Some("medium"),
        };
        let provider = GeminiProvider::new(None);
        let body_g3 = provider.build_request_body(&req_g3).unwrap();
        let json_g3: serde_json::Value = serde_json::from_str(&body_g3).unwrap();
        assert_eq!(
            json_g3["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "medium"
        );
        assert!(
            json_g3["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );

        // Test Gemini 2.x thinkingBudget
        let req_g2 = ChatRequest {
            messages: &messages,
            model: "gemini-2.0-flash",
            temperature: 0.7,
            max_tokens: None,
            tools: None,
            timeout_secs: 30,
            reasoning_effort: Some("high"),
        };
        let body_g2 = provider.build_request_body(&req_g2).unwrap();
        let json_g2: serde_json::Value = serde_json::from_str(&body_g2).unwrap();
        assert_eq!(
            json_g2["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            24576
        );
        assert_eq!(
            json_g2["generationConfig"]["thinkingConfig"]["includeThoughts"],
            true
        );
    }

    #[test]
    fn test_function_call_includes_thought_signature() {
        use crate::providers::{ChatMessage, FunctionCall, ToolCall};

        let mut messages = Vec::new();
        messages.push(ChatMessage::user("Check the weather"));

        // Assistant response with tool call (no thought_signature - simulating initial call)
        let tool_call_no_sig = ToolCall {
            id: "call_1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: "get_weather".to_string(),
                arguments: r#"{"location": "Paris"}"#.to_string(),
                thought_signature: None,
            },
        };
        let mut assistant_msg = ChatMessage::assistant("");
        assistant_msg.tool_calls = Some(vec![tool_call_no_sig]);
        messages.push(assistant_msg);

        // Tool response
        let tool_msg = ChatMessage {
            role: "tool".to_string(),
            content: r#"{"temperature": 22}"#.to_string(),
            name: Some("get_weather".to_string()),
            tool_call_id: Some("call_1".to_string()),
            tool_calls: None,
            content_parts: None,
            thought_signature: None,
        };
        messages.push(tool_msg);

        let req = ChatRequest {
            messages: &messages,
            model: "gemini-3.1-flash",
            temperature: 0.7,
            max_tokens: None,
            tools: None,
            timeout_secs: 30,
            reasoning_effort: None,
        };
        let provider = GeminiProvider::new(None);
        let body = provider.build_request_body(&req).unwrap();
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        // Verify the functionCall has a thought_signature (dummy fallback)
        let contents = json["contents"].as_array().unwrap();

        // Find ANY part with functionCall across all contents
        let mut found_sig = false;
        let mut found_dummy = false;
        for content in contents {
            if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                for part in parts {
                    if part.get("functionCall").is_some() {
                        found_sig = part.get("thought_signature").is_some();
                        if let Some(sig) = part.get("thought_signature").and_then(|s| s.as_str()) {
                            found_dummy = sig == "skip_thought_signature_validator";
                        }
                    }
                }
            }
        }

        assert!(
            found_sig,
            "functionCall should always include thought_signature"
        );
        assert!(found_dummy, "Should use dummy signature for initial call");
    }

    #[test]
    fn test_function_call_preserves_real_thought_signature() {
        use crate::providers::{ChatMessage, FunctionCall, ToolCall};

        let mut messages = Vec::new();
        messages.push(ChatMessage::user("Check the weather"));

        // Assistant response with tool call that HAS a real thought_signature
        let tool_call_with_sig = ToolCall {
            id: "call_1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: "get_weather".to_string(),
                arguments: r#"{"location": "Paris"}"#.to_string(),
                thought_signature: Some("real_signature_abc123".to_string()),
            },
        };
        let mut assistant_msg = ChatMessage::assistant("");
        assistant_msg.tool_calls = Some(vec![tool_call_with_sig]);
        messages.push(assistant_msg);

        let req = ChatRequest {
            messages: &messages,
            model: "gemini-3.1-flash",
            temperature: 0.7,
            max_tokens: None,
            tools: None,
            timeout_secs: 30,
            reasoning_effort: None,
        };
        let provider = GeminiProvider::new(None);
        let body = provider.build_request_body(&req).unwrap();
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        // Verify the functionCall has the real thought_signature
        let contents = json["contents"].as_array().unwrap();

        // Find ANY part with functionCall across all contents
        let mut found_real_sig = false;
        for content in contents {
            if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                for part in parts {
                    if part.get("functionCall").is_some() {
                        if let Some(sig) = part.get("thought_signature").and_then(|s| s.as_str()) {
                            found_real_sig = sig == "real_signature_abc123";
                        }
                    }
                }
            }
        }

        assert!(found_real_sig, "Should preserve real thought_signature");
    }

    #[test]
    fn test_multimodal_request_body_casing() {
        let mut messages = Vec::new();
        let parts = vec![
            crate::providers::ContentPart::Text("Look at this".to_string()),
            crate::providers::ContentPart::ImageBase64 {
                data: "base64data".to_string(),
                media_type: "image/png".to_string(),
            },
        ];
        let mut msg = crate::providers::ChatMessage::user("Look at this");
        msg.content_parts = Some(parts);
        messages.push(msg);

        let req = ChatRequest {
            messages: &messages,
            model: "gemini-1.5-flash",
            temperature: 0.7,
            max_tokens: None,
            tools: None,
            timeout_secs: 30,
            reasoning_effort: None,
        };

        let provider = GeminiProvider::new(None);
        let body = provider.build_request_body(&req).unwrap();
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        let contents = json["contents"].as_array().unwrap();
        let parts = contents[0]["parts"].as_array().unwrap();

        // Verify part 1: inlineData and mimeType (camelCase)
        let image_part = &parts[1];
        assert!(image_part.get("inlineData").is_some());
        assert!(image_part["inlineData"].get("mimeType").is_some());
        assert_eq!(image_part["inlineData"]["mimeType"], "image/png");
    }
}
