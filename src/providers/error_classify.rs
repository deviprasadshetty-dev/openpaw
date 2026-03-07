/// Classification of API errors for retry / fallback decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiErrorKind {
    /// 429 Too Many Requests — retry after backoff
    RateLimit,
    /// Quota exhausted — switch provider or give up
    Quota,
    /// 401 / 403 — bad key or permissions
    Auth,
    /// Model is unavailable or deprecated
    ModelDown,
    /// Network / timeout — retry quickly
    Network,
    /// Anything else
    Unknown,
}

impl ApiErrorKind {
    /// Should this error be retried?
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimit | Self::Network)
    }
}

/// Classify an error from HTTP status + response body text.
pub fn classify(status: u16, body: &str) -> ApiErrorKind {
    match status {
        401 | 403 => return ApiErrorKind::Auth,
        429 => {
            // Distinguish rate limit vs quota
            let lower = body.to_lowercase();
            if lower.contains("quota")
                || lower.contains("billing")
                || lower.contains("credit")
                || lower.contains("freeusagelimiterror")
            {
                return ApiErrorKind::Quota;
            }
            return ApiErrorKind::RateLimit;
        }
        503 | 529 => return ApiErrorKind::ModelDown,
        0 => return ApiErrorKind::Network, // timeout / connection refused
        _ => {}
    }

    // Parse JSON body for provider-specific error types
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        let err_type = v
            .pointer("/error/type")
            .or_else(|| v.pointer("/error/code"))
            .and_then(|t| t.as_str())
            .unwrap_or("");

        return match err_type {
            "rate_limit_exceeded" | "rate_limit" => ApiErrorKind::RateLimit,
            "insufficient_quota" | "billing_not_active" => ApiErrorKind::Quota,
            "authentication_error" | "permission_error" => ApiErrorKind::Auth,
            "model_not_found" | "model_unavailable" | "overloaded_error" => ApiErrorKind::ModelDown,
            _ => ApiErrorKind::Unknown,
        };
    }

    ApiErrorKind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_401_is_auth() {
        assert_eq!(classify(401, ""), ApiErrorKind::Auth);
    }

    #[test]
    fn test_429_rate_limit() {
        assert_eq!(
            classify(429, r#"{"error":{"type":"rate_limit_exceeded"}}"#),
            ApiErrorKind::RateLimit
        );
    }

    #[test]
    fn test_429_quota() {
        assert_eq!(classify(429, "insufficient quota"), ApiErrorKind::Quota);
    }

    #[test]
    fn test_retryable() {
        assert!(ApiErrorKind::RateLimit.is_retryable());
        assert!(ApiErrorKind::Network.is_retryable());
        assert!(!ApiErrorKind::Auth.is_retryable());
        assert!(!ApiErrorKind::Quota.is_retryable());
    }
}
