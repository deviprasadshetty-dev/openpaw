use crate::providers::{ChatRequest, ChatResponse, Provider};
use anyhow::Result;
use reqwest::blocking::Client;

use super::openai::OpenAiCompatibleProvider;

pub struct OpenRouterProvider {
    inner: OpenAiCompatibleProvider,
}

impl OpenRouterProvider {
    pub fn new(api_key: &str) -> Self {
        Self {
            inner: OpenAiCompatibleProvider {
                name: "openrouter".to_string(),
                base_url: "https://openrouter.ai/api/v1".to_string(),
                api_key: api_key.to_string(),
                client: Client::builder().build().unwrap(),
            },
        }
    }
}

impl Provider for OpenRouterProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        // OpenRouter uses the exact same Chat Completions API format as OpenAI.
        // The one difference is we might want to add site routing headers in the future,
        // but the base inner implementation is sufficient for now.
        self.inner.chat(request)
    }

    fn supports_native_tools(&self) -> bool {
        self.inner.supports_native_tools()
    }

    fn get_name(&self) -> &str {
        self.inner.get_name()
    }
}
