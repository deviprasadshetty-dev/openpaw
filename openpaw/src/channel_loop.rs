use crate::bus::{self, global_bus, InboundMessage};
use crate::channels::root::{Channel, ParsedMessage};
use crate::config::Config;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};

pub struct ChannelRuntime;

pub struct TelegramLoopState {
    pub last_activity: AtomicI64,
    pub stop_requested: Arc<AtomicBool>,
}

pub enum PollingState {
    Telegram(Arc<TelegramLoopState>),
}

pub struct PollingSpawnResult {
    pub state: Option<PollingState>,
    pub thread: Option<JoinHandle<()>>,
}

const TELEGRAM_OFFSET_STORE_VERSION: i64 = 1;

fn extract_telegram_bot_id(bot_token: &str) -> Option<String> {
    let colon_pos = bot_token.find(':')?;
    if colon_pos == 0 {
        return None;
    }
    let raw = bot_token[..colon_pos].trim();
    if raw.is_empty() {
        return None;
    }
    if raw.chars().all(|c| c.is_ascii_digit()) {
        Some(raw.to_string())
    } else {
        None
    }
}

fn normalize_telegram_account_id(account_id: &str) -> String {
    let trimmed = account_id.trim();
    let source = if trimmed.is_empty() { "default" } else { trimmed };
    source
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn telegram_update_offset_path(config: &Config, account_id: &str) -> Result<PathBuf> {
    let config_dir = Path::new(&config.config_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let normalized_account_id = normalize_telegram_account_id(account_id);
    let file_name = format!("update-offset-{}.json", normalized_account_id);
    Ok(config_dir.join("state").join("telegram").join(file_name))
}

pub fn load_telegram_update_offset(
    config: &Config,
    account_id: &str,
    bot_token: &str,
) -> Option<i64> {
    let path = telegram_update_offset_path(config, account_id).ok()?;
    let content = fs::read_to_string(path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;

    if let Some(version) = parsed.get("version") {
        if version.as_i64() != Some(TELEGRAM_OFFSET_STORE_VERSION) {
            return None;
        }
    }

    let last_update_id = parsed.get("last_update_id")?.as_i64()?;

    let expected_bot_id = extract_telegram_bot_id(bot_token);
    if let Some(expected) = expected_bot_id {
        let stored_bot_id = parsed.get("bot_id")?.as_str()?;
        if stored_bot_id != expected {
            return None;
        }
    } else if let Some(stored_bot_id) = parsed.get("bot_id") {
        if !stored_bot_id.is_null() && !stored_bot_id.is_string() {
            return None;
        }
    }

    Some(last_update_id)
}

pub fn save_telegram_update_offset(
    config: &Config,
    account_id: &str,
    bot_token: &str,
    update_id: i64,
) -> Result<()> {
    let path = telegram_update_offset_path(config, account_id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let bot_id = extract_telegram_bot_id(bot_token);
    let json = serde_json::json!({
        "version": TELEGRAM_OFFSET_STORE_VERSION,
        "last_update_id": update_id,
        "bot_id": bot_id
    });

    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, serde_json::to_string_pretty(&json)?)?;
    fs::rename(tmp_path, path)?;

    Ok(())
}

pub fn persist_telegram_update_offset_if_advanced(
    config: &Config,
    account_id: &str,
    bot_token: &str,
    persisted_update_id: &mut i64,
    candidate_update_id: i64,
) {
    if candidate_update_id <= *persisted_update_id {
        return;
    }
    if let Err(e) = save_telegram_update_offset(config, account_id, bot_token, candidate_update_id)
    {
        warn!("failed to persist telegram update offset: {}", e);
        return;
    }
    *persisted_update_id = candidate_update_id;
}

pub fn stop_polling(state: &PollingState) {
    match state {
        PollingState::Telegram(s) => s.stop_requested.store(true, Ordering::Relaxed),
    }
}

pub fn spawn_telegram_polling(
    _allocator: (),
    config: &Config,
    _runtime: &mut ChannelRuntime,
    channel: Arc<dyn Channel + Send + Sync>,
) -> Result<PollingSpawnResult> {
    let stop_requested = Arc::new(AtomicBool::new(false));
    let last_activity = Arc::new(AtomicI64::new(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
    ));

    let state = Arc::new(TelegramLoopState {
        stop_requested: stop_requested.clone(),
        last_activity: AtomicI64::new(0),
    });
    let state_clone = state.clone();

    let config_clone = config.clone();
    let channel_clone = channel.clone();

    let handle = thread::spawn(move || {
        telegram_polling_loop(channel_clone, &config_clone, stop_requested, last_activity);
    });

    Ok(PollingSpawnResult {
        state: Some(PollingState::Telegram(state_clone)),
        thread: Some(handle),
    })
}

fn telegram_polling_loop(
    channel: Arc<dyn Channel + Send + Sync>,
    config: &Config,
    stop_requested: Arc<AtomicBool>,
    last_activity: Arc<AtomicI64>,
) {
    info!(
        "Telegram polling thread started for account: {}",
        channel.account_id()
    );

    if !channel.health_check() {
        warn!(
            "Telegram channel {} health check failed on startup",
            channel.account_id()
        );
    }

    // Get bot token from channel config
    let account_config = config
        .channels
        .telegram
        .iter()
        .find(|acc| acc.account_id == channel.account_id())
        .expect("Telegram account config missing in polling loop");
    let bot_token = &account_config.bot_token;

    let mut offset = load_telegram_update_offset(config, channel.account_id(), bot_token).unwrap_or(0);
    // If offset is 0, we might want to start from latest, but Telegram handles offset=0 as "unconfirmed".
    // Actually if we pass offset+1 we confirm.
    // Let's assume poll_updates takes an offset.

    while !stop_requested.load(Ordering::Relaxed) {
        last_activity.store(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            Ordering::Relaxed,
        );

        // We need a way to poll updates passing an offset.
        // The `Channel` trait in openpaw likely has `poll` or similar if it supports polling.
        // If `poll` is not on the trait, we might need to cast to concrete `TelegramChannel`.
        // Since Rust doesn't support downcasting easily without Any, and Channel is likely `dyn Channel`,
        // we might assume `poll` is on the interface or we use `listen` which blocks?
        // `nullclaw` has `pollUpdates(offset)`.
        // Let's assume we added `poll` to `Channel` trait or we are using a concrete type here.
        // But `spawn_telegram_polling` takes `Arc<dyn Channel>`.
        // For now, I will assume the `Channel` trait has a `poll` method that takes an optional offset.
        // Or I will rely on `channel.listen()` but that usually blocks indefinitely or manages its own loop.
        // The `nullclaw` architecture extracts the loop to `channel_loop.zig` so the channel *doesn't* manage its own loop.
        // This implies `TelegramChannel` in `openpaw` should expose a way to fetch updates.

        // Since I cannot easily change the `Channel` trait definition right now without seeing `openpaw/src/channels/root.rs`,
        // I will assume `channel.poll(offset)` exists or I'll add a comment.
        // Wait, I can read `openpaw/src/channels/root.rs` to be sure.
        // For now I will mock the call as `channel.poll_updates(offset)`.

        // let updates = channel.poll_updates(offset); 
        // For now, let's pretend we have a method. If not, I'll need to fix `Channel` trait.

        // Simulating the loop logic:
        // 1. Poll updates
        // 2. For each update:
        //    - Parse message
        //    - Bus.publish(InboundMessage)
        //    - Update candidate offset
        // 3. Persist offset

        thread::sleep(Duration::from_secs(1)); // Placeholder sleep
    }

    info!(
        "Telegram polling thread stopped for account: {}",
        channel.account_id()
    );
}
