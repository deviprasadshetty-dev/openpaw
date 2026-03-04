use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,
    pub cooldown_duration: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            cooldown_duration: Duration::from_secs(30),
        }
    }
}

#[derive(Debug)]
struct CircuitBreakerState {
    state: CircuitState,
    failures: u32,
    last_failure_time: Option<Instant>,
}

#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: Arc<Mutex<CircuitBreakerState>>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(CircuitBreakerState {
                state: CircuitState::Closed,
                failures: 0,
                last_failure_time: None,
            })),
        }
    }

    pub fn allow_request(&self) -> bool {
        let mut state = self.state.lock().unwrap();

        match state.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(last_failure) = state.last_failure_time {
                    if last_failure.elapsed() >= self.config.cooldown_duration {
                        state.state = CircuitState::HalfOpen;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => false, // Only allow one test request (handled carefully by the caller or assuming caller checks and then executes one)
        }
    }

    // An alternative way to handle HalfOpen is allowing true if it just transitioned.
    pub fn check_and_attempt(&self) -> Result<(), &'static str> {
        let mut state = self.state.lock().unwrap();

        match state.state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => {
                if let Some(last_failure) = state.last_failure_time {
                    if last_failure.elapsed() >= self.config.cooldown_duration {
                        state.state = CircuitState::HalfOpen;
                        Ok(())
                    } else {
                        Err("Circuit breaker is OPEN")
                    }
                } else {
                    Err("Circuit breaker is OPEN")
                }
            }
            CircuitState::HalfOpen => {
                // In half-open, we typically allow one request through. If this returns Ok, we are allowing another.
                // For simplicity, we can let the caller handle it or allow a trickle.
                // Let's allow one concurrent attempt.
                Ok(())
            }
        }
    }

    pub fn record_success(&self) {
        let mut state = self.state.lock().unwrap();
        state.failures = 0;
        state.state = CircuitState::Closed;
    }

    pub fn record_failure(&self) {
        let mut state = self.state.lock().unwrap();
        state.failures += 1;
        state.last_failure_time = Some(Instant::now());

        if state.failures >= self.config.failure_threshold {
            state.state = CircuitState::Open;
        } else if state.state == CircuitState::HalfOpen {
            // If it fails during half-open, immediately trip back to open
            state.state = CircuitState::Open;
        }
    }
}
