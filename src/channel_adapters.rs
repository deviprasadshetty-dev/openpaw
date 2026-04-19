use crate::channel_loop::{self, ChannelRuntime, PollingSpawnResult};
use crate::channels::root::Channel;
use crate::channels::telegram::TelegramChannel;
use crate::config::Config;
use crate::config_types::{ChatType, PeerRef};
use anyhow::Result;
use std::sync::Arc;

pub type PollingSpawnFn = fn(
    allocator: (),
    config: &Config,
    runtime: &mut ChannelRuntime,
    channel: Arc<dyn Channel + Send + Sync>,
) -> Result<PollingSpawnResult>;

pub type PollingSourceKeyFn = fn(allocator: (), channel: &dyn Channel) -> Option<String>;

pub struct PollingDescriptor {
    pub channel_name: &'static str,
    pub spawn: PollingSpawnFn,
    pub source_key: Option<PollingSourceKeyFn>,
}

fn telegram_polling_source_key(_allocator: (), channel: &dyn Channel) -> Option<String> {
    if let Some(tg) = channel.as_any().downcast_ref::<TelegramChannel>() {
        // Return account_id as the source key for distinguishing multiple Telegram bots
        return Some(tg.account_id().to_string());
    }
    None
}

pub const POLLING_DESCRIPTORS: &[PollingDescriptor] = &[PollingDescriptor {
    channel_name: "telegram",
    spawn: channel_loop::spawn_telegram_polling,
    source_key: Some(telegram_polling_source_key),
}];

pub fn find_polling_descriptor(channel_name: &str) -> Option<&'static PollingDescriptor> {
    POLLING_DESCRIPTORS
        .iter()
        .find(|desc| desc.channel_name == channel_name)
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InboundMetadata {
    pub account_id: Option<String>,
    pub peer_kind: Option<ChatType>,
    pub peer_id: Option<String>,
    pub message_id: Option<String>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
    pub channel_id: Option<String>,
    pub thread_id: Option<String>,
    pub is_dm: Option<bool>,
    pub is_group: Option<bool>,
}

pub struct InboundRouteInput {
    pub channel_name: String,
    pub sender_id: String,
    pub chat_id: String,
}

pub type MatchesFn = fn(config: &Config, channel_name: &str) -> bool;
pub type DefaultAccountIdFn = fn(config: &Config, channel_name: &str) -> Option<String>;
pub type DerivePeerFn = fn(input: &InboundRouteInput, meta: &InboundMetadata) -> Option<PeerRef>;

pub struct InboundRouteDescriptor {
    pub channel_name: Option<&'static str>,
    pub matches_fn: Option<MatchesFn>,
    pub default_account_id: DefaultAccountIdFn,
    pub derive_peer: DerivePeerFn,
}

pub fn parse_peer_kind(raw: &str) -> Option<ChatType> {
    match raw {
        "direct" => Some(ChatType::Direct),
        "group" => Some(ChatType::Group),
        "channel" => Some(ChatType::Channel),
        _ => None,
    }
}

// Default account helpers removed for non-Telegram channels

// Peer derivation helpers removed for non-Telegram channels
