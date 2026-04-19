use crate::config::Config;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelId {
    Cli,
    Telegram,
    Webhook,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListenerMode {
    None,
    Polling,
    GatewayLoop,
    WebhookOnly,
    SendOnly,
}

pub struct ChannelMeta {
    pub id: ChannelId,
    pub key: &'static str,
    pub label: &'static str,
    pub configured_message: &'static str,
    pub listener_mode: ListenerMode,
}

pub const KNOWN_CHANNELS: &[ChannelMeta] = &[
    ChannelMeta {
        id: ChannelId::Cli,
        key: "cli",
        label: "CLI",
        configured_message: "CLI enabled",
        listener_mode: ListenerMode::None,
    },
    ChannelMeta {
        id: ChannelId::Telegram,
        key: "telegram",
        label: "Telegram",
        configured_message: "Telegram configured",
        listener_mode: ListenerMode::Polling,
    },
    ChannelMeta {
        id: ChannelId::Webhook,
        key: "webhook",
        label: "Webhook",
        configured_message: "Webhook configured",
        listener_mode: ListenerMode::None,
    },
];

// Placeholder for build options
mod build_options {
    pub const ENABLE_CHANNEL_CLI: bool = true;
    pub const ENABLE_CHANNEL_TELEGRAM: bool = true;
}

pub fn is_build_enabled(channel_id: ChannelId) -> bool {
    match channel_id {
        ChannelId::Cli => build_options::ENABLE_CHANNEL_CLI,
        ChannelId::Telegram => build_options::ENABLE_CHANNEL_TELEGRAM,
        ChannelId::Webhook => true,
    }
}

// NOTE: Config struct in crate::config does not have 'channels' field yet.
// We need to implement full Config structure to support configured_count.
// For now, returning 0 or placeholder.
pub fn configured_count(_cfg: &Config, _channel_id: ChannelId) -> usize {
    // TODO: Implement actual config check when Config struct is fully ported
    0
}

pub fn is_configured(cfg: &Config, channel_id: ChannelId) -> bool {
    is_build_enabled(channel_id) && configured_count(cfg, channel_id) > 0
}

pub fn find_by_key(key: &str) -> Option<&'static ChannelMeta> {
    KNOWN_CHANNELS.iter().find(|meta| meta.key == key)
}

pub fn find_by_id(id: ChannelId) -> Option<&'static ChannelMeta> {
    KNOWN_CHANNELS.iter().find(|meta| meta.id == id)
}

pub fn contributes_to_daemon_supervision(channel_id: ChannelId) -> bool {
    if let Some(meta) = find_by_id(channel_id) {
        meta.listener_mode != ListenerMode::None
    } else {
        false
    }
}

pub fn requires_runtime(channel_id: ChannelId) -> bool {
    if let Some(meta) = find_by_id(channel_id) {
        matches!(
            meta.listener_mode,
            ListenerMode::Polling | ListenerMode::GatewayLoop | ListenerMode::WebhookOnly
        )
    } else {
        false
    }
}
