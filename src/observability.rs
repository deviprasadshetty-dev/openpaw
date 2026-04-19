use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::{Arc, Mutex};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ObserverEvent {
    AgentStart {
        provider: String,
        model: String,
    },
    LlmRequest {
        provider: String,
        model: String,
        messages_count: usize,
    },
    LlmResponse {
        provider: String,
        model: String,
        duration_ms: u64,
        success: bool,
        error_message: Option<String>,
    },
    AgentEnd {
        duration_ms: u64,
        tokens_used: Option<u64>,
    },
    ToolCallStart {
        tool: String,
    },
    ToolCall {
        tool: String,
        duration_ms: u64,
        success: bool,
        detail: Option<String>,
    },
    ToolIterationsExhausted {
        iterations: u32,
    },
    TurnComplete,
    ChannelMessage {
        channel: String,
        direction: String,
    },
    HeartbeatTick,
    Error {
        component: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ObserverMetric {
    RequestLatencyMs(u64),
    TokensUsed(u64),
    ActiveSessions(u64),
    QueueDepth(u64),
}

pub trait Observer: Send + Sync {
    fn record_event(&self, event: &ObserverEvent);
    fn record_metric(&self, metric: &ObserverMetric);
    fn flush(&self);
    fn name(&self) -> &str;
}

pub struct NoopObserver;

impl Observer for NoopObserver {
    fn record_event(&self, _event: &ObserverEvent) {}
    fn record_metric(&self, _metric: &ObserverMetric) {}
    fn flush(&self) {}
    fn name(&self) -> &str {
        "noop"
    }
}

pub struct LogObserver;

impl Observer for LogObserver {
    fn record_event(&self, event: &ObserverEvent) {
        match event {
            ObserverEvent::AgentStart { provider, model } => {
                info!("agent.start provider={} model={}", provider, model)
            }
            ObserverEvent::LlmRequest {
                provider,
                model,
                messages_count,
            } => info!(
                "llm.request provider={} model={} messages={}",
                provider, model, messages_count
            ),
            ObserverEvent::LlmResponse {
                provider,
                model,
                duration_ms,
                success,
                error_message: _,
            } => info!(
                "llm.response provider={} model={} duration_ms={} success={}",
                provider, model, duration_ms, success
            ),
            ObserverEvent::AgentEnd {
                duration_ms,
                tokens_used: _,
            } => info!("agent.end duration_ms={}", duration_ms),
            ObserverEvent::ToolCallStart { tool } => info!("tool.start tool={}", tool),
            ObserverEvent::ToolCall {
                tool,
                duration_ms,
                success,
                detail,
            } => {
                if let Some(d) = detail {
                    info!(
                        "tool.call tool={} duration_ms={} success={} detail={}",
                        tool, duration_ms, success, d
                    )
                } else {
                    info!(
                        "tool.call tool={} duration_ms={} success={}",
                        tool, duration_ms, success
                    )
                }
            }
            ObserverEvent::ToolIterationsExhausted { iterations } => {
                info!("tool.iterations_exhausted iterations={}", iterations)
            }
            ObserverEvent::TurnComplete => info!("turn.complete"),
            ObserverEvent::ChannelMessage { channel, direction } => {
                info!(
                    "channel.message channel={} direction={}",
                    channel, direction
                )
            }
            ObserverEvent::HeartbeatTick => info!("heartbeat.tick"),
            ObserverEvent::Error { component, message } => {
                info!("error component={} message={}", component, message)
            }
        }
    }

    fn record_metric(&self, metric: &ObserverMetric) {
        match metric {
            ObserverMetric::RequestLatencyMs(v) => {
                info!("metric.request_latency latency_ms={}", v)
            }
            ObserverMetric::TokensUsed(v) => info!("metric.tokens_used tokens={}", v),
            ObserverMetric::ActiveSessions(v) => {
                info!("metric.active_sessions sessions={}", v)
            }
            ObserverMetric::QueueDepth(v) => info!("metric.queue_depth depth={}", v),
        }
    }

    fn flush(&self) {}
    fn name(&self) -> &str {
        "log"
    }
}

pub struct VerboseObserver {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl Default for VerboseObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl VerboseObserver {
    pub fn new() -> Self {
        Self {
            writer: Arc::new(Mutex::new(Box::new(std::io::stderr()))),
        }
    }
}

impl Observer for VerboseObserver {
    fn record_event(&self, event: &ObserverEvent) {
        let mut w = self.writer.lock().unwrap();
        match event {
            ObserverEvent::LlmRequest {
                provider,
                model,
                messages_count,
            } => {
                let _ = writeln!(w, "> Thinking");
                let _ = writeln!(
                    w,
                    "> Send (provider={}, model={}, messages={})",
                    provider, model, messages_count
                );
            }
            ObserverEvent::LlmResponse {
                provider: _,
                model: _,
                duration_ms,
                success,
                error_message: _,
            } => {
                let _ = writeln!(
                    w,
                    "< Receive (success={}, duration_ms={})",
                    success, duration_ms
                );
            }
            ObserverEvent::ToolCallStart { tool } => {
                let _ = writeln!(w, "> Tool {}", tool);
            }
            ObserverEvent::ToolCall {
                tool,
                duration_ms,
                success,
                detail,
            } => {
                if let Some(d) = detail {
                    let _ = writeln!(
                        w,
                        "< Tool {} (success={}, duration_ms={}, detail={})",
                        tool, success, duration_ms, d
                    );
                } else {
                    let _ = writeln!(
                        w,
                        "< Tool {} (success={}, duration_ms={})",
                        tool, success, duration_ms
                    );
                }
            }
            ObserverEvent::TurnComplete => {
                let _ = writeln!(w, "< Complete");
            }
            _ => {}
        }
    }

    fn record_metric(&self, _metric: &ObserverMetric) {}
    fn flush(&self) {
        let mut w = self.writer.lock().unwrap();
        let _ = w.flush();
    }
    fn name(&self) -> &str {
        "verbose"
    }
}
