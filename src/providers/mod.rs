pub mod anthropic;
pub mod circuit_breaker;
pub mod error_classify;
pub mod factory;
pub mod fallback;
pub mod gemini;
pub mod kilocode;
pub mod ollama;
pub mod openai;
pub mod openrouter;
pub mod reliable;
pub mod sse;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Content part for multimodal messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPart {
    /// Plain text content
    Text(String),
    /// Base64-encoded image data
    #[serde(rename = "image_base64")]
    ImageBase64 { data: String, media_type: String },
    /// Image URL (may not be supported by all providers)
    #[serde(rename = "image_url")]
    ImageUrl { url: String },
    /// Generic media data (multimodal)
    Media { mime_type: String, data: Vec<u8> },
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
            thought_signature: None,
        }
    }

    /// Create an assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
            thought_signature: None,
        }
    }

    /// Create a system message
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
            thought_signature: None,
        }
    }

    /// Create a message with content parts (multimodal)
    pub fn with_content_parts(parts: Vec<ContentPart>) -> Self {
        Self {
            role: "user".to_string(),
            content: String::new(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: Some(parts),
            thought_signature: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(&self) -> &str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub content_parts: Option<Vec<ContentPart>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String, // "function"
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: TokenUsage,
    pub model: String,
    pub reasoning_content: Option<String>,
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ChatRequest<'a> {
    pub messages: &'a [ChatMessage],
    pub model: &'a str,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    pub tools: Option<&'a [ToolSpec]>,
    pub timeout_secs: u64,
    pub reasoning_effort: Option<&'a str>,
}

// ── Streaming types ─────────────────────────────────────────────

/// A chunk emitted during streaming LLM responses.
#[derive(Debug, Clone)]
pub enum StreamChunk {
    /// Incremental text content from the model.
    Delta(String),
    /// Incremental tool call data (for native tool-calling providers).
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: String,
    },
    /// Stream finished — carries final token usage.
    Done(TokenUsage),
    /// An error occurred mid-stream.
    Error(String),
}

/// Callback type for streaming responses. Called once per chunk.
pub type StreamCallback = Box<dyn FnMut(StreamChunk) + Send>;

use std::sync::atomic::{AtomicU32, Ordering};

pub struct SharedRetryState {
    pub total_attempts: AtomicU32,
    pub max_total: u32,
}

impl SharedRetryState {
    pub fn new(max_total: u32) -> Self {
        Self {
            total_attempts: AtomicU32::new(0),
            max_total,
        }
    }

    pub fn can_retry(&self) -> bool {
        self.total_attempts.load(Ordering::Relaxed) < self.max_total
    }

    pub fn increment_attempts(&self) -> u32 {
        self.total_attempts.fetch_add(1, Ordering::Relaxed) + 1
    }
}

pub trait Provider: Send + Sync {
    /// Send a chat request to the LLM (blocking, returns full response)
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse>;

    /// Send a chat request with shared retry awareness
    fn chat_with_retry(
        &self,
        request: &ChatRequest,
        _shared_retry: &std::sync::Arc<SharedRetryState>,
    ) -> Result<ChatResponse> {
        self.chat(request)
    }

    /// Send a chat request and stream deltas via callback.
    /// Default implementation falls back to blocking `chat()`.
    fn chat_stream(
        &self,
        request: &ChatRequest,
        mut callback: StreamCallback,
    ) -> Result<ChatResponse> {
        let response = self.chat(request)?;
        if let Some(ref text) = response.content {
            callback(StreamChunk::Delta(text.clone()));
        }
        callback(StreamChunk::Done(response.usage.clone()));
        Ok(response)
    }

    /// Whether this provider inherently supports returning native tool calls
    fn supports_native_tools(&self) -> bool;

    /// Whether the current model supports multimodal input (images, audio, video).
    /// Default is true since most modern models support some form of multimodal.
    /// Override to return false for models known to lack vision/multimodal support.
    fn supports_multimodal(&self) -> bool {
        true
    }

    /// Provider name for diagnostics
    fn get_name(&self) -> &str;
}
