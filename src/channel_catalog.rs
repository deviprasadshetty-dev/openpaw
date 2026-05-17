use crate::config::Config;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelId {
    Cli,
    Telegram,
    Email,
    WhatsAppNative,
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
        id: ChannelId::Email,
        key: "email",
        label: "Email",
        configured_message: "Email configured",
        listener_mode: ListenerMode::Polling,
    },
    ChannelMeta {
        id: ChannelId::WhatsAppNative,
        key: "whatsapp_native",
        label: "WhatsApp Native",
        configured_message: "WhatsApp Native configured",
        listener_mode: ListenerMode::GatewayLoop,
    },
    ChannelMeta {
        id: ChannelId::Webhook,
        key: "webhook",
        label: "Webhook",
        configured_message: "Webhook configured",
        listener_mode: ListenerMode::None,
    },
];

// Placeholder for build options — uses cfg! to respect feature flags
mod build_options {
    pub const ENABLE_CHANNEL_CLI: bool = cfg!(feature = "cli") || true; // always on
    pub const ENABLE_CHANNEL_TELEGRAM: bool = cfg!(feature = "telegram");
    pub const ENABLE_CHANNEL_EMAIL: bool = cfg!(feature = "email");
    pub const ENABLE_CHANNEL_WHATSAPP_NATIVE: bool = cfg!(feature = "whatsapp");
}

pub fn is_build_enabled(channel_id: ChannelId) -> bool {
    match channel_id {
        ChannelId::Cli => build_options::ENABLE_CHANNEL_CLI,
        ChannelId::Telegram => build_options::ENABLE_CHANNEL_TELEGRAM,
        ChannelId::Email => build_options::ENABLE_CHANNEL_EMAIL,
        ChannelId::WhatsAppNative => build_options::ENABLE_CHANNEL_WHATSAPP_NATIVE,
        ChannelId::Webhook => true,
    }
}

pub fn configured_count(cfg: &Config, channel_id: ChannelId) -> usize {
    match channel_id {
        ChannelId::Cli => usize::from(cfg.channels.cli),
        ChannelId::Telegram => cfg.channels.telegram.len(),
        ChannelId::Email => cfg.channels.email.len(),
        ChannelId::WhatsAppNative => cfg.channels.whatsapp_native.len(),
        ChannelId::Webhook => usize::from(cfg.channels.webhook.is_some()),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_count_reflects_configured_channels() {
        let mut cfg = crate::config_parse::parse_config("{}", "./config.json", false).unwrap();
        cfg.channels.cli = true;
        cfg.channels
            .telegram
            .push(crate::config_types::TelegramConfig {
                account_id: "main".to_string(),
                bot_token: "token".to_string(),
                allow_from: vec![],
                group_allow_from: vec![],
                group_policy: "allowlist".to_string(),
                reply_in_private: true,
                proxy: None,
                webhook_url: None,
            });
        cfg.channels.email.push(crate::config_types::EmailConfig {
            account_id: "mail".to_string(),
            smtp_user: "user".to_string(),
            smtp_pass: "pass".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 587,
            imap_host: "imap.example.com".to_string(),
            imap_port: 993,
        });
        cfg.channels
            .whatsapp_native
            .push(crate::config_types::WhatsAppNativeConfig {
                account_id: "wa".to_string(),
                bridge_url: "http://127.0.0.1:3001".to_string(),
                allow_from: vec![],
                auto_start: false,
                bridge_dir: None,
            });
        cfg.channels.webhook = Some(crate::config_types::WebhookConfig {});

        assert_eq!(configured_count(&cfg, ChannelId::Cli), 1);
        assert_eq!(configured_count(&cfg, ChannelId::Telegram), 1);
        assert_eq!(configured_count(&cfg, ChannelId::Email), 1);
        assert_eq!(configured_count(&cfg, ChannelId::WhatsAppNative), 1);
        assert_eq!(configured_count(&cfg, ChannelId::Webhook), 1);
    }
}
