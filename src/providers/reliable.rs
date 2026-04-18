/// ReliableProvider — wraps any Provider with retry + exponential backoff.
///
/// Only retries on `RateLimit` and `Network` errors.
/// Auth and Quota errors are returned immediately.
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use tracing::warn;

use super::circuit_breaker::CircuitBreaker;
use crate::providers::error_classify::classify;
use crate::providers::{ChatRequest, ChatResponse, Provider, SharedRetryState};

pub struct ReliableProvider {
    inner: Arc<dyn Provider>,
    max_retries: u32,
    /// Initial backoff in ms (doubles each attempt, capped at max_backoff_ms)
    initial_backoff_ms: u64,
    max_backoff_ms: u64,
    circuit_breaker: CircuitBreaker,
}

impl ReliableProvider {
    pub fn new(inner: Arc<dyn Provider>) -> Self {
        Self {
            inner,
            max_retries: 3,
            initial_backoff_ms: 1000,
            max_backoff_ms: 30_000,
            circuit_breaker: crate::providers::circuit_breaker::CircuitBreaker::new(
                Default::default(),
            ),
        }
    }

    pub fn with_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }
}

impl Provider for ReliableProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let shared_retry = std::sync::Arc::new(SharedRetryState::new(self.max_retries));
        self.chat_with_retry(request, &shared_retry)
    }

    fn chat_with_retry(
        &self,
        request: &ChatRequest,
        shared_retry: &std::sync::Arc<SharedRetryState>,
    ) -> Result<ChatResponse> {
        let mut backoff_ms = self.initial_backoff_ms;

        loop {
            if let Err(e) = self.circuit_breaker.check_and_attempt() {
                return Err(anyhow::anyhow!("{}", e));
            }

            match self.inner.chat_with_retry(request, shared_retry) {
                Ok(resp) => {
                    self.circuit_breaker.record_success();
                    return Ok(resp);
                }
                Err(e) => {
                    self.circuit_breaker.record_failure();
                    let msg = e.to_string();
                    let status = extract_status_from_message(&msg);
                    let kind = classify(status, &msg);

                    if !kind.is_retryable() || !shared_retry.can_retry() {
                        warn!(
                            provider = self.inner.get_name(),
                            error_kind = ?kind,
                            "Non-retryable error or max retries reached: {}",
                            msg
                        );
                        return Err(e);
                    }

                    let attempt = shared_retry.increment_attempts();
                    warn!(
                        provider = self.inner.get_name(),
                        attempt, backoff_ms, "Retryable error ({:?}), backing off: {}", kind, msg
                    );
                    thread::sleep(Duration::from_millis(backoff_ms));
                    backoff_ms = (backoff_ms * 2).min(self.max_backoff_ms);
                }
            }
        }
    }

    fn supports_native_tools(&self) -> bool {
        self.inner.supports_native_tools()
    }

    fn get_name(&self) -> &str {
        self.inner.get_name()
    }

    fn chat_stream(
        &self,
        request: &ChatRequest,
        callback: crate::providers::StreamCallback,
    ) -> Result<ChatResponse> {
        if let Err(e) = self.circuit_breaker.check_and_attempt() {
            return Err(anyhow::anyhow!("{}", e));
        }

        // B-5: Don't automatically fall back to blocking chat, as stream callback is consumed.
        match self.inner.chat_stream(request, callback) {
            Ok(resp) => {
                self.circuit_breaker.record_success();
                Ok(resp)
            }
            Err(e) => {
                self.circuit_breaker.record_failure();
                Err(e)
            }
        }
    }
}

/// Try to extract an HTTP status code from an anyhow error message string.
/// Returns 0 if not found (treated as Network error).
fn extract_status_from_message(msg: &str) -> u16 {
    // Patterns like "API error 429:" or "status: 429"
    for part in msg.split_whitespace() {
        if let Ok(n) = part.trim_end_matches(':').parse::<u16>()
            && (100..=599).contains(&n) {
                return n;
            }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_status() {
        assert_eq!(extract_status_from_message("API error 429: too many"), 429);
        assert_eq!(extract_status_from_message("connection refused"), 0);
    }
}
