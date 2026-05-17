/// Classification of API errors for retry / fallback decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiErrorKind {
    /// 429 Too Many Requests — retry after backoff
    RateLimit { retry_after_secs: Option<u64> },
    /// Quota exhausted (402/429 with billing) — switch provider or give up
    Quota,
    /// 401 / 403 — bad key or permissions
    Auth,
    /// Model is unavailable or deprecated
    ModelDown,
    /// Network / timeout — retry quickly
    Network,
    /// Context length exceeded — compress and retry
    ContextOverflow { limit: Option<u64>, available_output: Option<u64> },
    /// Payment required (402) — credit exhausted, switch provider
    CreditExhausted,
    /// Anything else
    Unknown,
}

impl ApiErrorKind {
    /// Should this error be retried?
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimit { .. } | Self::Network | Self::ContextOverflow { .. })
    }

    /// Should we switch to a fallback provider?
    pub fn needs_fallback(&self) -> bool {
        matches!(self, Self::Quota | Self::CreditExhausted | Self::ModelDown | Self::Auth)
    }
}

/// Structured error info extracted from an API error response.
#[derive(Debug, Clone)]
pub struct ErrorInfo {
    pub kind: ApiErrorKind,
    pub retry_after_secs: Option<u64>,
    pub context_limit: Option<u64>,
    pub available_output_tokens: Option<u64>,
}

impl Default for ErrorInfo {
    fn default() -> Self {
        Self {
            kind: ApiErrorKind::Unknown,
            retry_after_secs: None,
            context_limit: None,
            available_output_tokens: None,
        }
    }
}

/// Classify an error from HTTP status + response body text.
/// Returns full ErrorInfo with extracted structured fields.
pub fn classify_full(status: u16, body: &str) -> ErrorInfo {
    let mut info = ErrorInfo::default();
    let lower = body.to_lowercase();

    // Extract retry-after from headers/body
    info.retry_after_secs = parse_retry_after(body);

    // Extract context limit
    info.context_limit = crate::token_estimator::parse_context_limit_from_error(body);

    // Extract available output tokens
    info.available_output_tokens =
        crate::token_estimator::parse_available_output_tokens_from_error(body);

    match status {
        401 | 403 => {
            info.kind = ApiErrorKind::Auth;
            return info;
        }
        402 => {
            info.kind = ApiErrorKind::CreditExhausted;
            return info;
        }
        429 => {
            // Distinguish rate limit vs quota
            if lower.contains("quota")
                || lower.contains("billing")
                || lower.contains("credit")
                || lower.contains("freeusagelimiterror")
            {
                info.kind = ApiErrorKind::Quota;
                return info;
            }
            info.kind = ApiErrorKind::RateLimit {
                retry_after_secs: info.retry_after_secs,
            };
            return info;
        }
        503 | 529 => {
            info.kind = ApiErrorKind::ModelDown;
            return info;
        }
        0 => {
            info.kind = ApiErrorKind::Network;
            return info;
        }
        _ => {}
    }

    // Parse JSON body for provider-specific error types
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        let err_type = v
            .pointer("/error/type")
            .or_else(|| v.pointer("/error/code"))
            .and_then(|t| t.as_str())
            .unwrap_or("");

        // Also check nested error structures (Anthropic, OpenAI, etc.)
        let nested_error = v
            .pointer("/error/error/type")
            .or_else(|| v.pointer("/error/message"))
            .and_then(|t| t.as_str())
            .unwrap_or("");

        // Check for context overflow patterns in error body
        if lower.contains("context_length_exceeded")
            || lower.contains("context window")
            || lower.contains("maximum context")
            || lower.contains("input is too long")
            || lower.contains("reduce the length")
            || err_type.contains("context_length")
            || nested_error.contains("context_length")
        {
            info.kind = ApiErrorKind::ContextOverflow {
                limit: info.context_limit,
                available_output: info.available_output_tokens,
            };
            return info;
        }

        // Check for credit/billing exhaustion
        if lower.contains("insufficient_balance")
            || lower.contains("payment required")
            || lower.contains("billing")
            || lower.contains("credit balance")
            || lower.contains("ran out of credits")
            || err_type == "insufficient_quota"
            || err_type == "billing_not_active"
        {
            info.kind = ApiErrorKind::CreditExhausted;
            return info;
        }

        info.kind = match err_type {
            "rate_limit_exceeded" | "rate_limit" => ApiErrorKind::RateLimit {
                retry_after_secs: info.retry_after_secs,
            },
            "insufficient_quota" | "billing_not_active" => ApiErrorKind::Quota,
            "authentication_error" | "permission_error" => ApiErrorKind::Auth,
            "model_not_found" | "model_unavailable" | "overloaded_error" => ApiErrorKind::ModelDown,
            _ => ApiErrorKind::Unknown,
        };
        return info;
    }

    info.kind = ApiErrorKind::Unknown;
    info
}

/// Simple classification (backward compatible).
pub fn classify(status: u16, body: &str) -> ApiErrorKind {
    classify_full(status, body).kind
}

/// Try to extract Retry-After seconds from error body or headers.
fn parse_retry_after(body: &str) -> Option<u64> {
    let lower = body.to_lowercase();

    // Common patterns: "Retry-After: 30", "retry_after": 30, "try again in 30s"
    let patterns = [
        r"retry[-_ ]after[:\s]+(\d+)",
        r"try again in (\d+)\s*(?:s|sec|second)",
        r"retry in (\d+)\s*(?:s|sec|second)",
        r"wait (\d+)\s*(?:s|sec|second)",
        r"available in (\d+)\s*(?:s|sec|second)",
    ];

    for pat in patterns {
        if let Ok(re) = regex::Regex::new(pat) {
            if let Some(cap) = re.captures(&lower) {
                if let Ok(secs) = cap[1].parse::<u64>() {
                    if secs > 0 && secs <= 3600 {
                        return Some(secs);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_401_is_auth() {
        assert_eq!(classify(401, ""), ApiErrorKind::Auth);
    }

    #[test]
    fn test_402_is_credit_exhausted() {
        assert_eq!(classify(402, ""), ApiErrorKind::CreditExhausted);
    }

    #[test]
    fn test_429_rate_limit() {
        assert_eq!(
            classify(429, r#"{"error":{"type":"rate_limit_exceeded"}}"#),
            ApiErrorKind::RateLimit { retry_after_secs: None }
        );
    }

    #[test]
    fn test_429_rate_limit_with_retry_after() {
        let info = classify_full(429, "Retry-After: 30\nrate limit exceeded");
        assert!(matches!(info.kind, ApiErrorKind::RateLimit { .. }));
        assert_eq!(info.retry_after_secs, Some(30));
    }

    #[test]
    fn test_429_quota() {
        assert_eq!(classify(429, "insufficient quota"), ApiErrorKind::Quota);
    }

    #[test]
    fn test_context_overflow_detection() {
        let info = classify_full(400, "context_length_exceeded: maximum context length is 32768 tokens");
        assert!(matches!(info.kind, ApiErrorKind::ContextOverflow { .. }));
        assert_eq!(info.context_limit, Some(32768));
    }

    #[test]
    fn test_retryable() {
        assert!(ApiErrorKind::RateLimit { retry_after_secs: None }.is_retryable());
        assert!(ApiErrorKind::Network.is_retryable());
        assert!(ApiErrorKind::ContextOverflow { limit: None, available_output: None }.is_retryable());
        assert!(!ApiErrorKind::Auth.is_retryable());
        assert!(!ApiErrorKind::Quota.is_retryable());
        assert!(!ApiErrorKind::CreditExhausted.is_retryable());
    }

    #[test]
    fn test_needs_fallback() {
        assert!(ApiErrorKind::Quota.needs_fallback());
        assert!(ApiErrorKind::CreditExhausted.needs_fallback());
        assert!(ApiErrorKind::ModelDown.needs_fallback());
        assert!(ApiErrorKind::Auth.needs_fallback());
        assert!(!ApiErrorKind::Network.needs_fallback());
    }

    #[test]
    fn test_parse_retry_after() {
        assert_eq!(parse_retry_after("Retry-After: 42"), Some(42));
        assert_eq!(parse_retry_after("try again in 15s"), Some(15));
        assert_eq!(parse_retry_after("retry in 5 seconds"), Some(5));
        assert_eq!(parse_retry_after("wait 60 sec"), Some(60));
        assert_eq!(parse_retry_after("no retry info here"), None);
    }
}
