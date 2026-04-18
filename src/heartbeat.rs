use crate::bus::{Bus, InboundMessage};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HeartbeatState {
    pub last_runs: HashMap<String, u64>,
}

pub struct HeartbeatEngine {
    pub enabled: bool,
    pub interval_minutes: u32,
    pub workspace_dir: PathBuf,
    pub bus: Arc<Bus>,
}

pub struct TickResult {
    pub outcome: Outcome,
    pub task_count: usize,
}

pub enum Outcome {
    Processed,
    SkippedEmptyFile,
    SkippedMissingFile,
}

impl HeartbeatEngine {
    pub fn init(enabled: bool, interval_minutes: u32, workspace_dir: &str, bus: Arc<Bus>) -> Self {
        Self {
            enabled,
            interval_minutes,
            workspace_dir: PathBuf::from(workspace_dir),
            bus,
        }
    }

    fn state_file(&self) -> PathBuf {
        self.workspace_dir.join(".heartbeat_state.json")
    }

    fn load_state(&self) -> HeartbeatState {
        let path = self.state_file();
        if let Ok(content) = fs::read_to_string(path) {
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HeartbeatState::default()
        }
    }

    fn save_state(&self, state: &HeartbeatState) {
        let path = self.state_file();
        if let Ok(content) = serde_json::to_string_pretty(state) {
            let _ = fs::write(path, content);
        }
    }

    pub fn tick(&self) -> Result<TickResult> {
        if !self.enabled {
            return Ok(TickResult {
                outcome: Outcome::SkippedEmptyFile,
                task_count: 0,
            });
        }

        let heartbeat_file = self.workspace_dir.join("HEARTBEAT.md");
        if !heartbeat_file.exists() {
            debug!("HEARTBEAT.md missing");
            return Ok(TickResult {
                outcome: Outcome::SkippedMissingFile,
                task_count: 0,
            });
        }

        let content = fs::read_to_string(&heartbeat_file)?;
        if content.trim().is_empty() {
            return Ok(TickResult {
                outcome: Outcome::SkippedEmptyFile,
                task_count: 0,
            });
        }

        let mut state = self.load_state();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut tasks_triggered = 0;

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("- [ ]") {
                let task_part = line[5..].trim();
                if let Some(at_idx) = task_part.find("@every") {
                    let description = task_part[..at_idx].trim();
                    let interval_str = task_part[at_idx + 6..].trim();

                    if let Ok(interval_secs) = crate::cron::parse_duration(interval_str) {
                        let last_run = state.last_runs.get(description).cloned().unwrap_or(0);
                        if now >= last_run + (interval_secs as u64) {
                            info!("Heartbeat triggering proactive task: {}", description);

                            let msg = InboundMessage {
                                channel: "internal".to_string(),
                                sender_id: "heartbeat".to_string(),
                                chat_id: "proactive".to_string(),
                                content: format!("PROACTIVE TASK CHECK: {}\n\nPlease check on this and take any necessary action.", description),
                                session_key: "internal:proactive".to_string(),
                                media: Vec::new(),
                                metadata_json: None,
                                task_kind: Some("heartbeat".to_string()),
                                is_group: false,
                            };

                            if let Err(e) = self.bus.publish_inbound(msg) {
                                warn!("Failed to publish heartbeat task to bus: {}", e);
                            } else {
                                state.last_runs.insert(description.to_string(), now);
                                tasks_triggered += 1;
                            }
                        }
                    }
                }
            }
        }

        if tasks_triggered > 0 {
            self.save_state(&state);
        }

        Ok(TickResult {
            outcome: Outcome::Processed,
            task_count: tasks_triggered,
        })
    }
}
