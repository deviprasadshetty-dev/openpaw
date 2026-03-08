use anyhow::Result;
use std::sync::Arc;
use tracing::{info, warn};

use crate::providers::error_classify::classify;
use crate::providers::{ChatRequest, ChatResponse, Provider, SharedRetryState, StreamCallback};

pub struct FallbackProvider {
    providers: Vec<Arc<dyn Provider>>,
}

impl FallbackProvider {
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        Self { providers }
    }
}

impl Provider for FallbackProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let shared_retry = std::sync::Arc::new(SharedRetryState::new(3)); // Default 3 total attempts
        self.chat_with_retry(request, &shared_retry)
    }

    fn chat_with_retry(
        &self,
        request: &ChatRequest,
        shared_retry: &std::sync::Arc<SharedRetryState>,
    ) -> Result<ChatResponse> {
        let mut last_err = anyhow::anyhow!("No providers configured for fallback");

        for (i, provider) in self.providers.iter().enumerate() {
            if !shared_retry.can_retry() && i > 0 {
                break;
            }

            match provider.chat_with_retry(request, shared_retry) {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let msg = e.to_string();
                    let status = extract_status_from_message(&msg);
                    let kind = classify(status, &msg);

                    if kind.is_retryable()
                        || matches!(kind, crate::providers::error_classify::ApiErrorKind::Quota)
                    {
                        warn!(
                            provider = provider.get_name(),
                            "Provider failed with {:?}, trying fallback {}/{}",
                            kind,
                            i + 1,
                            self.providers.len()
                        );
                        last_err = e;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        Err(last_err)
    }

    fn chat_stream(&self, request: &ChatRequest, callback: StreamCallback) -> Result<ChatResponse> {
        // Fallback for streaming is complex because callback is consumed.
        // Try first provider with stream, if it fails, fall back to non-streaming chat_with_retry.

        if let Some(provider) = self.providers.first() {
            match provider.chat_stream(request, callback) {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let msg = e.to_string();
                    let status = extract_status_from_message(&msg);
                    let kind = classify(status, &msg);

                    if kind.is_retryable()
                        || matches!(kind, crate::providers::error_classify::ApiErrorKind::Quota)
                    {
                        info!(
                            provider = provider.get_name(),
                            "Stream failed with {:?}, falling back to non-streaming rotation...",
                            kind
                        );
                        let shared_retry = std::sync::Arc::new(SharedRetryState::new(3));
                        return self.chat_with_retry(request, &shared_retry);
                    }
                    return Err(e);
                }
            }
        }

        Err(anyhow::anyhow!("All providers failed in fallback"))
    }

    fn supports_native_tools(&self) -> bool {
        // Fallback provider supports native tools if the first provider does.
        // This is a simplification; ideally we'd filter providers based on request needs.
        self.providers
            .first()
            .map(|p| p.supports_native_tools())
            .unwrap_or(false)
    }

    fn get_name(&self) -> &str {
        "fallback"
    }
}

fn extract_status_from_message(msg: &str) -> u16 {
    for part in msg.split_whitespace() {
        if let Ok(n) = part.trim_end_matches(':').parse::<u16>() {
            if (100..=599).contains(&n) {
                return n;
            }
        }
    }
    0
}
