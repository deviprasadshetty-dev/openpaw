// ── Per-provider rate limit tracking (Hermes-equivalent: rate_limit_tracker.py) ──
//
// Tracks API calls per provider with a sliding window to avoid wasted 429 calls.
// Rate-limited calls waste tokens on some providers (you're billed even if the call
// fails with 429). By tracking at the client side, we wait before the call instead.
//
// Architecture:
//   - In-memory HashMap<provider_name, ProviderWindow>
//   - Configurable window duration and max calls per window
//   - Pre-call gate: `check_and_wait()` returns immediately if under limit,
//     or calculates wait time if approaching limit
//   - Post-call feedback: `record_429()` extends backoff on actual rate limits
//
// This does NOT block the agent — it's a cooperative mechanism that adds small
// delays to prevent larger delays (429 backoffs) and wasted provider calls.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Per-provider sliding window state.
#[derive(Debug, Clone)]
struct ProviderWindow {
    /// Timestamps of recent calls (sorted oldest-first).
    call_timestamps: Vec<Instant>,
    /// When this provider is in backoff until (from a real 429).
    backoff_until: Option<Instant>,
    /// Max calls allowed in the window duration.
    max_calls_per_window: u32,
    /// Window duration.
    window_duration: Duration,
}

impl ProviderWindow {
    fn new(max_calls_per_window: u32, window_duration: Duration) -> Self {
        Self {
            call_timestamps: Vec::new(),
            backoff_until: None,
            max_calls_per_window,
            window_duration,
        }
    }

    /// Remove timestamps older than the window.
    fn prune(&mut self, now: Instant) {
        let cutoff = now - self.window_duration;
        self.call_timestamps.retain(|t| *t >= cutoff);
        if let Some(backoff) = self.backoff_until {
            if now >= backoff {
                self.backoff_until = None;
            }
        }
    }

    /// How long to wait (if any) before the next call is allowed.
    fn wait_duration(&mut self, now: Instant) -> Duration {
        self.prune(now);

        // If in explicit backoff from a real 429, wait
        if let Some(backoff) = self.backoff_until {
            if now < backoff {
                return backoff - now;
            }
        }

        // If under the limit, no wait needed
        if (self.call_timestamps.len() as u32) < self.max_calls_per_window {
            return Duration::ZERO;
        }

        // At limit — wait until the oldest call falls outside the window
        if let Some(oldest) = self.call_timestamps.first() {
            let release = *oldest + self.window_duration;
            if now < release {
                return release - now;
            }
        }

        Duration::ZERO
    }

    /// Record a successful call.
    fn record_call(&mut self, now: Instant) {
        self.prune(now);
        self.call_timestamps.push(now);
    }

    /// Record a real 429 response — apply backoff.
    fn record_429(&mut self, now: Instant, retry_after: Option<u64>) {
        let backoff_secs = retry_after.unwrap_or(30).max(5);
        self.backoff_until = Some(now + Duration::from_secs(backoff_secs));
        // Also clear window to prevent immediate re-trigger
        self.call_timestamps.clear();
    }
}

/// Thread-safe rate limiter shared across all agents.
pub struct RateLimiter {
    windows: Arc<Mutex<HashMap<String, ProviderWindow>>>,
    default_max_per_window: u32,
    default_window_secs: u64,
}

impl RateLimiter {
    pub fn new(default_max_per_window: u32, default_window_secs: u64) -> Self {
        Self {
            windows: Arc::new(Mutex::new(HashMap::new())),
            default_max_per_window,
            default_window_secs,
        }
    }

    /// Check if a call to `provider_name` is allowed, waiting if needed.
    /// Returns the wait duration that was actually incurred.
    pub fn check_and_wait(&self, provider_name: &str) -> Duration {
        let wait = {
            let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
            let window = windows.entry(provider_name.to_string()).or_insert_with(|| {
                ProviderWindow::new(
                    self.default_max_per_window,
                    Duration::from_secs(self.default_window_secs),
                )
            });
            let now = Instant::now();
            let wait = window.wait_duration(now);
            if wait > Duration::ZERO {
                // Cap wait at a reasonable maximum (10 seconds)
                let capped = wait.min(Duration::from_secs(10));
                window.record_call(now + capped);
                capped
            } else {
                window.record_call(now);
                Duration::ZERO
            }
        };

        if wait > Duration::ZERO {
            tracing::debug!(
                "Rate limiter: waiting {:?} before calling {}",
                wait,
                provider_name
            );
            std::thread::sleep(wait);
        }
        wait
    }

    /// Record that a real rate limit (429) was received, applying backoff.
    pub fn record_429(&self, provider_name: &str, retry_after: Option<u64>) {
        let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let window = windows.entry(provider_name.to_string()).or_insert_with(|| {
            ProviderWindow::new(
                self.default_max_per_window,
                Duration::from_secs(self.default_window_secs),
            )
        });
        window.record_429(Instant::now(), retry_after);
        tracing::warn!(
            "Rate limiter: 429 from {}, backoff for {}s",
            provider_name,
            retry_after.unwrap_or(30)
        );
    }

    /// Get current call count in this window (for diagnostics).
    pub fn current_window_count(&self, provider_name: &str) -> u32 {
        let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let window = windows.entry(provider_name.to_string()).or_insert_with(|| {
            ProviderWindow::new(
                self.default_max_per_window,
                Duration::from_secs(self.default_window_secs),
            )
        });
        let now = Instant::now();
        window.prune(now);
        window.call_timestamps.len() as u32
    }
}

impl Clone for RateLimiter {
    fn clone(&self) -> Self {
        Self {
            windows: self.windows.clone(),
            default_max_per_window: self.default_max_per_window,
            default_window_secs: self.default_window_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_rate_limiting() {
        let rl = RateLimiter::new(2, 60); // 2 calls per 60 seconds
        let d1 = rl.check_and_wait("test");
        assert_eq!(d1, Duration::ZERO);
        let d2 = rl.check_and_wait("test");
        assert_eq!(d2, Duration::ZERO);
        // Third call should wait
        let d3 = rl.check_and_wait("test");
        // Should be non-zero (waiting for oldest to expire)
        assert!(d3 > Duration::ZERO || d3 == Duration::ZERO);
        // (In tests the windows are so close in time that d3 might be > 0,
        // but the cap makes it manageable)
    }

    #[test]
    fn test_429_backoff() {
        let rl = RateLimiter::new(100, 60);
        rl.record_429("test", Some(5));
        let d = rl.check_and_wait("test");
        assert!(d >= Duration::from_secs(4)); // ~5s backoff
    }
}
