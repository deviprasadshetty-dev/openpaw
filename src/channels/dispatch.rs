use crate::bus::{Bus, OutboundMessage};
use crate::channels::root::Channel;
use crate::streaming::OutboundStage;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{error, info, warn};

// ════════════════════════════════════════════════════════════════════════════
// Supervised Channel (for health checking and restart)
// ════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct SupervisedChannel {
    pub name: String,
    pub account_id: String,
    pub restart_count: u64,
    pub healthy: bool,
}

impl SupervisedChannel {
    pub fn new(name: String, account_id: String) -> Self {
        Self {
            name,
            account_id,
            restart_count: 0,
            healthy: true,
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Channel Registry
// ════════════════════════════════════════════════════════════════════════════

pub struct ChannelEntry {
    pub channel: Arc<dyn Channel + Send + Sync>,
    pub account_id: String,
}

pub struct ChannelRegistry {
    pub channels: Vec<ChannelEntry>,
}

pub struct HealthReport {
    pub healthy: usize,
    pub unhealthy: usize,
    pub total: usize,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
        }
    }

    pub fn register(&mut self, ch: Arc<dyn Channel + Send + Sync>) {
        let account_id = ch.account_id().to_string();
        self.channels.push(ChannelEntry {
            channel: ch,
            account_id,
        });
    }

    pub fn register_with_account(&mut self, ch: Arc<dyn Channel + Send + Sync>, account_id: &str) {
        self.channels.push(ChannelEntry {
            channel: ch,
            account_id: account_id.to_string(),
        });
    }

    pub fn count(&self) -> usize {
        self.channels.len()
    }

    pub fn find_by_name(&self, channel_name: &str) -> Option<&(dyn Channel + Send + Sync)> {
        for entry in &self.channels {
            if entry.channel.name() == channel_name {
                return Some(entry.channel.as_ref());
            }
        }
        None
    }

    pub fn find_by_name_account(
        &self,
        channel_name: &str,
        account_id: &str,
    ) -> Option<&(dyn Channel + Send + Sync)> {
        for entry in &self.channels {
            if entry.channel.name() == channel_name && entry.account_id == account_id {
                return Some(entry.channel.as_ref());
            }
        }
        None
    }

    pub fn health_check_all(&self) -> HealthReport {
        let mut healthy = 0;
        let mut unhealthy = 0;
        for entry in &self.channels {
            if entry.channel.health_check() {
                healthy += 1;
            } else {
                unhealthy += 1;
            }
        }
        HealthReport {
            healthy,
            unhealthy,
            total: self.channels.len(),
        }
    }
}

pub fn build_system_prompt(
    base_prompt: &str,
    channel_name: &str,
    identity_name: &str,
) -> String {
    format!(
        "{}\n\nYou are {}. You are responding on the {} channel.",
        base_prompt, identity_name, channel_name
    )
}

// ════════════════════════════════════════════════════════════════════════════
// Outbound Dispatch Loop
// ════════════════════════════════════════════════════════════════════════════

pub struct DispatchStats {
    pub dispatched: AtomicU64,
    pub errors: AtomicU64,
    pub channel_not_found: AtomicU64,
}

impl Default for DispatchStats {
    fn default() -> Self {
        Self {
            dispatched: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            channel_not_found: AtomicU64::new(0),
        }
    }
}

pub fn run_outbound_dispatcher(
    bus: Arc<Bus>,
    registry: Arc<ChannelRegistry>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let stats = DispatchStats::default();
        info!("Outbound dispatcher started");

        loop {
            match bus.consume_outbound() {
                Some(msg) => {
                    dispatch_message(&registry, &msg, &stats);
                }
                None => {
                    // Channel closed, exit
                    info!("Outbound channel closed, dispatcher exiting");
                    break;
                }
            }
        }
    })
}

fn dispatch_message(
    registry: &ChannelRegistry,
    msg: &OutboundMessage,
    stats: &DispatchStats,
) {
    // Find the appropriate channel
    let channel = if let Some(ref account_id) = msg.account_id {
        registry.find_by_name_account(&msg.channel, account_id)
    } else {
        registry.find_by_name(&msg.channel)
    };

    let channel = match channel {
        Some(ch) => ch,
        None => {
            warn!(
                "No channel found for {} (account: {:?})",
                msg.channel, msg.account_id
            );
            stats
                .channel_not_found
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    // Send the message
    if let Err(e) = channel.send_message(&msg.chat_id, &msg.content) {
        error!("Failed to send message to {}: {}", msg.channel, e);
        stats.errors.fetch_add(1, Ordering::Relaxed);
    } else {
        stats.dispatched.fetch_add(1, Ordering::Relaxed);

        // Log chunk vs final for debugging
        match msg.stage {
            OutboundStage::Chunk => {
                info!("Dispatched chunk to {} ({})", msg.channel, msg.chat_id);
            }
            OutboundStage::Final => {
                info!("Dispatched final message to {} ({})", msg.channel, msg.chat_id);
            }
        }
    }
}

pub fn run_dispatcher_with_backoff(
    bus: Arc<Bus>,
    registry: Arc<ChannelRegistry>,
    stop_requested: Arc<AtomicU64>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let stats = DispatchStats::default();
        let mut backoff_ms: u64 = 0;
        const MAX_BACKOFF_MS: u64 = 5000;

        loop {
            // Check stop request
            if stop_requested.load(Ordering::Relaxed) > 0 {
                info!("Outbound dispatcher stopping (requested)");
                break;
            }

            match bus.consume_outbound() {
                Some(msg) => {
                    // Reset backoff on successful receive
                    backoff_ms = 0;
                    dispatch_message(&registry, &msg, &stats);
                }
                None => {
                    // No message available, apply backoff
                    if backoff_ms < MAX_BACKOFF_MS {
                        backoff_ms = (backoff_ms * 2).max(10);
                    }
                    thread::sleep(Duration::from_millis(backoff_ms));
                }
            }
        }
    })
}
