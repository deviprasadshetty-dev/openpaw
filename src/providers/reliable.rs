/// ReliableProvider — wraps any Provider with retry + exponential backoff.
///
/// Only retries on `RateLimit` and `Network` errors.
/// Auth and Quota errors are returned immediately.
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use tracing::warn;

use crate::providers::error_classify::classify;
use crate::providers::{ChatRequest, ChatResponse, Provider};

pub struct ReliableProvider {
    inner: Arc<dyn Provider>,
    max_retries: u32,
    /// Initial backoff in ms (doubles each attempt, capped at max_backoff_ms)
    initial_backoff_ms: u64,
    max_backoff_ms: u64,
}

impl ReliableProvider {
    pub fn new(inner: Arc<dyn Provider>) -> Self {
        Self {
            inner,
            max_retries: 3,
            initial_backoff_ms: 1000,
            max_backoff_ms: 30_000,
        }
    }

    pub fn with_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }
}

impl Provider for ReliableProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let mut backoff_ms = self.initial_backoff_ms;
        let mut last_err = anyhow::anyhow!("No attempts made");

        for attempt in 0..=self.max_retries {
            match self.inner.chat(request) {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let msg = e.to_string();
                    // Extract HTTP status from error message if available
                    let status = extract_status_from_message(&msg);
                    let kind = classify(status, &msg);

                    if !kind.is_retryable() || attempt == self.max_retries {
                        warn!(
                            provider = self.inner.get_name(),
                            attempt,
                            error_kind = ?kind,
                            "Non-retryable error or max retries reached: {}",
                            msg
                        );
                        return Err(e);
                    }

                    warn!(
                        provider = self.inner.get_name(),
                        attempt, backoff_ms, "Retryable error ({:?}), backing off: {}", kind, msg
                    );
                    thread::sleep(Duration::from_millis(backoff_ms));
                    backoff_ms = (backoff_ms * 2).min(self.max_backoff_ms);
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    fn supports_native_tools(&self) -> bool {
        self.inner.supports_native_tools()
    }

    fn get_name(&self) -> &str {
        self.inner.get_name()
    }
}

/// Try to extract an HTTP status code from an anyhow error message string.
/// Returns 0 if not found (treated as Network error).
fn extract_status_from_message(msg: &str) -> u16 {
    // Patterns like "API error 429:" or "status: 429"
    for part in msg.split_whitespace() {
        if let Ok(n) = part.trim_end_matches(':').parse::<u16>() {
            if (100..=599).contains(&n) {
                return n;
            }
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
