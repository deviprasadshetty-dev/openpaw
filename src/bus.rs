use crate::streaming::OutboundStage;
use crossbeam_channel::{Receiver, SendError, Sender, bounded};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InboundMessage {
    pub channel: String,     // "telegram", "discord", "webhook", "system"
    pub sender_id: String,   // sender identifier
    pub chat_id: String,     // chat/room identifier
    pub content: String,     // message text
    pub session_key: String, // "channel:chatID" for session lookup
    #[serde(default)]
    pub media: Vec<String>, // file paths/URLs (images, voice, docs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_json: Option<String>, // channel-specific JSON (message_id, thread_ts, is_group)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OutboundMessage {
    pub channel: String, // target channel
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>, // target account (multi-account channels)
    pub chat_id: String, // target chat
    pub content: String, // response text
    #[serde(default)]
    pub media: Vec<String>, // file paths/URLs to send
    pub stage: OutboundStage,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn make_inbound(
    channel: &str,
    sender_id: &str,
    chat_id: &str,
    content: &str,
    session_key: &str,
) -> InboundMessage {
    InboundMessage {
        channel: channel.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        content: content.to_string(),
        session_key: session_key.to_string(),
        media: Vec::new(),
        metadata_json: None,
    }
}

pub fn make_inbound_full(
    channel: &str,
    sender_id: &str,
    chat_id: &str,
    content: &str,
    session_key: &str,
    media_src: &[String],
    metadata_json: Option<&str>,
) -> InboundMessage {
    InboundMessage {
        channel: channel.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        content: content.to_string(),
        session_key: session_key.to_string(),
        media: media_src.to_vec(),
        metadata_json: metadata_json.map(|s| s.to_string()),
    }
}

pub fn make_outbound(channel: &str, chat_id: &str, content: &str) -> OutboundMessage {
    make_outbound_with_stage(channel, chat_id, content, OutboundStage::Final)
}

pub fn make_outbound_chunk(channel: &str, chat_id: &str, content: &str) -> OutboundMessage {
    make_outbound_with_stage(channel, chat_id, content, OutboundStage::Chunk)
}

fn make_outbound_with_stage(
    channel: &str,
    chat_id: &str,
    content: &str,
    stage: OutboundStage,
) -> OutboundMessage {
    OutboundMessage {
        channel: channel.to_string(),
        account_id: None,
        chat_id: chat_id.to_string(),
        content: content.to_string(),
        media: Vec::new(),
        stage,
    }
}

pub fn make_outbound_with_account(
    channel: &str,
    account_id: &str,
    chat_id: &str,
    content: &str,
) -> OutboundMessage {
    make_outbound_with_account_stage(channel, account_id, chat_id, content, OutboundStage::Final)
}

pub fn make_outbound_chunk_with_account(
    channel: &str,
    account_id: &str,
    chat_id: &str,
    content: &str,
) -> OutboundMessage {
    make_outbound_with_account_stage(channel, account_id, chat_id, content, OutboundStage::Chunk)
}

fn make_outbound_with_account_stage(
    channel: &str,
    account_id: &str,
    chat_id: &str,
    content: &str,
    stage: OutboundStage,
) -> OutboundMessage {
    OutboundMessage {
        channel: channel.to_string(),
        account_id: Some(account_id.to_string()),
        chat_id: chat_id.to_string(),
        content: content.to_string(),
        media: Vec::new(),
        stage,
    }
}

pub fn make_outbound_with_media(
    channel: &str,
    chat_id: &str,
    content: &str,
    media_src: &[String],
) -> OutboundMessage {
    OutboundMessage {
        channel: channel.to_string(),
        account_id: None,
        chat_id: chat_id.to_string(),
        content: content.to_string(),
        media: media_src.to_vec(),
        stage: OutboundStage::Final,
    }
}

// ---------------------------------------------------------------------------
// Bus - top-level structure
// ---------------------------------------------------------------------------

pub const QUEUE_CAPACITY: usize = 100;

#[derive(Debug, Clone)]
pub struct Bus {
    inbound_tx: Sender<InboundMessage>,
    inbound_rx: Receiver<InboundMessage>,
    outbound_tx: Sender<OutboundMessage>,
    outbound_rx: Receiver<OutboundMessage>,
}

impl Bus {
    pub fn new() -> Self {
        let (inbound_tx, inbound_rx) = bounded(QUEUE_CAPACITY);
        let (outbound_tx, outbound_rx) = bounded(QUEUE_CAPACITY);
        Self {
            inbound_tx,
            inbound_rx,
            outbound_tx,
            outbound_rx,
        }
    }

    // -- Inbound: channels/gateway -> agent --

    #[allow(clippy::result_large_err)]
    pub fn publish_inbound(&self, msg: InboundMessage) -> Result<(), SendError<InboundMessage>> {
        self.inbound_tx.send(msg)
    }

    pub fn consume_inbound(&self) -> Option<InboundMessage> {
        self.inbound_rx.recv().ok()
    }

    pub fn consume_inbound_timeout(&self, timeout: std::time::Duration) -> Option<InboundMessage> {
        self.inbound_rx.recv_timeout(timeout).ok()
    }

    // -- Outbound: agent/cron/heartbeat -> channels --

    #[allow(clippy::result_large_err)]
    pub fn publish_outbound(&self, msg: OutboundMessage) -> Result<(), SendError<OutboundMessage>> {
        self.outbound_tx.send(msg)
    }

    pub fn consume_outbound(&self) -> Option<OutboundMessage> {
        self.outbound_rx.recv().ok()
    }

    // -- Lifecycle --

    // In Rust/crossbeam, dropping the Senders closes the channel.
    // Explicit close is usually not needed if we drop the Senders,
    // but if `Bus` holds them, we might need a method to drop them.
    // However, `Sender` doesn't have a `close` method. `Receiver` doesn't either.
    // The channel closes when all Senders are dropped.
    // Since `Bus` holds `inbound_tx` and `outbound_tx`, we can't easily "close" it
    // without dropping `Bus` or wrapping Senders in Option and taking them out.
    // Or we can just let it be. Zig's manual close is to unblock waiters.
    // Crossbeam handles this automatically on drop.

    // If manual closing is required while Bus is alive (e.g. for shutdown signal),
    // we would need to redesign Bus to hold Option<Sender> or similar.
    // For now, I'll assume standard RAII is enough or I can implement a close by using `Option`.
    // Actually, `Sender` is clonable, so even if we drop `Bus`'s sender, others might exist.
    // But `Bus` seems to be the owner/coordinator.

    // Zig's `close` sets a flag and broadcasts condition variable.
    // I won't implement explicit close for now as standard Rust patterns rely on Drop.
    // If specific shutdown logic is needed, we'd need to know more about ownership.

    // -- Metrics --

    pub fn inbound_depth(&self) -> usize {
        self.inbound_rx.len()
    }

    pub fn outbound_depth(&self) -> usize {
        self.outbound_rx.len()
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Global Bus Instance
// ---------------------------------------------------------------------------

use std::sync::OnceLock;

static GLOBAL_BUS: OnceLock<Bus> = OnceLock::new();

/// Initialize the global bus instance
pub fn init_global_bus() -> &'static Bus {
    GLOBAL_BUS.get_or_init(Bus::new)
}

/// Get the global bus instance (must call init_global_bus first)
pub fn global_bus() -> Option<&'static Bus> {
    GLOBAL_BUS.get()
}

// Message enum for the bus
#[derive(Debug, Clone)]
pub enum Message {
    Inbound(InboundMessage),
    Outbound(OutboundMessage),
}
