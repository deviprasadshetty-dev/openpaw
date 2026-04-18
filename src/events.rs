/// Reactive event trigger system.
///
/// Agents and tools can `emit` named events with a payload.
/// Registered watchers fire when an event name matches their pattern.
/// Watcher actions are either an agent task prompt (published as an inbound bus message)
/// or a shell command executed in the workspace.
use crate::bus::{Bus, make_inbound, make_outbound};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs, path::PathBuf};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WatcherAction {
    /// Post the prompt as an inbound message to the main agent.
    AgentTask {
        prompt: String,
        /// Optional named agent profile (matches config.agents[].name).
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
    },
    /// Run a shell command in the workspace directory.
    ShellCommand { command: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventWatcher {
    pub id: u64,
    /// Event name pattern. Use `*` as a wildcard suffix (e.g. `"file.*"` matches `"file.changed"`).
    pub event_pattern: String,
    pub action: WatcherAction,
    pub label: String,
    pub origin_channel: String,
    pub origin_chat_id: String,
    pub created_at: u64,
    pub fire_count: u64,
}

pub struct EventRegistry {
    watchers: Arc<Mutex<HashMap<u64, EventWatcher>>>,
    next_id: Arc<Mutex<u64>>,
    bus: Arc<Bus>,
    workspace_dir: String,
}

fn watchers_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".openpaw").join("event_watchers.json")
}

impl EventRegistry {
    pub fn new(workspace_dir: &str, bus: Arc<Bus>) -> Self {
        let registry = Self {
            watchers: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            bus,
            workspace_dir: workspace_dir.to_string(),
        };
        registry.load();
        registry
    }

    fn load(&self) {
        let path = watchers_path();
        if let Ok(data) = fs::read_to_string(&path) {
            if let Ok(watchers) = serde_json::from_str::<HashMap<u64, EventWatcher>>(&data) {
                let max_id = watchers.keys().max().copied().unwrap_or(0);
                *self.next_id.lock().unwrap_or_else(|e| e.into_inner()) = max_id + 1;
                *self.watchers.lock().unwrap_or_else(|e| e.into_inner()) = watchers;
            }
        }
    }

    fn save(&self) {
        let path = watchers_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let guard = self.watchers.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(data) = serde_json::to_string_pretty(&*guard) {
            let _ = fs::write(path, data);
        }
    }

    /// Register a watcher. Returns the watcher ID.
    pub fn watch(
        &self,
        event_pattern: &str,
        action: WatcherAction,
        label: &str,
        origin_channel: &str,
        origin_chat_id: &str,
    ) -> u64 {
        let mut id_guard = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
        let id = *id_guard;
        *id_guard += 1;
        drop(id_guard);

        let watcher = EventWatcher {
            id,
            event_pattern: event_pattern.to_string(),
            action,
            label: label.to_string(),
            origin_channel: origin_channel.to_string(),
            origin_chat_id: origin_chat_id.to_string(),
            created_at: now_secs(),
            fire_count: 0,
        };
        self.watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, watcher);
        self.save();
        id
    }

    /// Remove a watcher by ID. Returns true if it existed.
    pub fn unwatch(&self, watcher_id: u64) -> bool {
        let removed = self.watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&watcher_id)
            .is_some();
        if removed {
            self.save();
        }
        removed
    }

    /// List all registered watchers.
    pub fn list(&self) -> Vec<EventWatcher> {
        let guard = self.watchers.lock().unwrap_or_else(|e| e.into_inner());
        guard.values().cloned().collect()
    }

    /// Emit an event, firing all matching watchers.
    /// Returns the number of watchers triggered.
    pub fn emit(
        &self,
        event_name: &str,
        payload: &str,
        emitter_channel: &str,
        emitter_chat_id: &str,
    ) -> usize {
        let mut guard = self.watchers.lock().unwrap_or_else(|e| e.into_inner());
        let mut triggered = 0usize;

        for watcher in guard.values_mut() {
            if !pattern_matches(&watcher.event_pattern, event_name) {
                continue;
            }
            watcher.fire_count += 1;
            triggered += 1;

            let origin_channel = watcher.origin_channel.clone();
            let origin_chat_id = watcher.origin_chat_id.clone();

            match &watcher.action {
                WatcherAction::AgentTask { prompt, .. } => {
                    // Inject event context into the prompt
                    let full_prompt = format!(
                        "[Event: {}] {}\n\nEvent payload: {}",
                        event_name, prompt, payload
                    );
                    let session_key = format!("{}:{}", origin_channel, origin_chat_id);
                    let inbound = make_inbound(
                        &origin_channel,
                        "event_system",
                        &origin_chat_id,
                        &full_prompt,
                        &session_key,
                    );
                    if let Err(e) = self.bus.publish_inbound(inbound) {
                        warn!("EventRegistry: failed to publish inbound for watcher {}: {}", watcher.id, e);
                    } else {
                        info!(
                            "EventRegistry: event '{}' triggered agent task for watcher {}",
                            event_name, watcher.id
                        );
                    }
                }
                WatcherAction::ShellCommand { command } => {
                    let command = command.clone();
                    let workspace = self.workspace_dir.clone();
                    let bus = self.bus.clone();
                    let watcher_id = watcher.id;
                    let ev_name = event_name.to_string();
                    let payload_str = payload.to_string();
                    // Clone before moving into the async block so we can still use them below
                    let shell_origin_channel = origin_channel.clone();
                    let shell_origin_chat = origin_chat_id.clone();

                    // Fire shell command in a background task
                    tokio::spawn(async move {
                        let output = if cfg!(windows) {
                            let full_cmd = format!(
                                "set EVENT_NAME={}&& set EVENT_PAYLOAD={}&& {}",
                                ev_name, payload_str, command
                            );
                            tokio::process::Command::new("cmd")
                                .arg("/c")
                                .arg(&full_cmd)
                                .current_dir(&workspace)
                                .output()
                                .await
                        } else {
                            let full_cmd = format!(
                                "EVENT_NAME='{}' EVENT_PAYLOAD='{}' {}",
                                ev_name, payload_str, command
                            );
                            tokio::process::Command::new("sh")
                                .arg("-c")
                                .arg(&full_cmd)
                                .current_dir(&workspace)
                                .output()
                                .await
                        };

                        let report = match output {
                            Ok(out) => {
                                let stdout = String::from_utf8_lossy(&out.stdout);
                                let stderr = String::from_utf8_lossy(&out.stderr);
                                format!(
                                    "⚡ Event '{}' shell watcher {} completed (exit: {}):\n{}{}",
                                    ev_name,
                                    watcher_id,
                                    out.status.code().unwrap_or(-1),
                                    if stdout.is_empty() { String::new() } else { format!("stdout: {}", stdout) },
                                    if stderr.is_empty() { String::new() } else { format!("\nstderr: {}", stderr) },
                                )
                            }
                            Err(e) => format!(
                                "⚡ Event '{}' shell watcher {} failed: {}",
                                ev_name, watcher_id, e
                            ),
                        };
                        let outbound = make_outbound(&shell_origin_channel, &shell_origin_chat, &report);
                        let _ = bus.publish_outbound(outbound);
                    });
                }
            }

            // Also notify the emitter that the watcher fired (if different from origin)
            if emitter_channel != origin_channel || emitter_chat_id != origin_chat_id {
                let ack = format!(
                    "⚡ Event '{}' triggered watcher '{}' (ID {})",
                    event_name, watcher.label, watcher.id
                );
                let outbound = make_outbound(emitter_channel, emitter_chat_id, &ack);
                let _ = self.bus.publish_outbound(outbound);
            }
        }

        if triggered > 0 {
            self.save();
        }

        triggered
    }
}

/// Simple glob pattern matching: supports `*` as a wildcard only at the end.
/// e.g. `"file.*"` matches `"file.changed"` and `"file.deleted"`.
fn pattern_matches(pattern: &str, event: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix(".*") {
        // Match anything starting with "{prefix}."
        return event.starts_with(&format!("{}.", prefix));
    }
    if pattern.ends_with('*') {
        let prefix = &pattern[..pattern.len() - 1];
        return event.starts_with(prefix);
    }
    pattern == event
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_matches() {
        assert!(pattern_matches("file.*", "file.changed"));
        assert!(pattern_matches("file.*", "file.deleted"));
        assert!(!pattern_matches("file.*", "network.up"));
        assert!(pattern_matches("*", "anything"));
        assert!(pattern_matches("exact", "exact"));
        assert!(!pattern_matches("exact", "exactplus"));
        assert!(pattern_matches("pre*", "prefix.event"));
    }
}
