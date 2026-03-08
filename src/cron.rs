use crate::bus::Bus;
use crate::config::Config;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobType {
    Shell,
    Agent,
}

impl Default for JobType {
    fn default() -> Self {
        Self::Shell
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTarget {
    Isolated,
    Main,
}

impl Default for SessionTarget {
    fn default() -> Self {
        Self::Isolated
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub expression: String,
    pub command: String,
    #[serde(default)]
    pub next_run_secs: i64,
    pub last_run_secs: Option<i64>,
    pub last_status: Option<String>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub one_shot: bool,
    #[serde(default)]
    pub job_type: JobType,
    #[serde(default)]
    pub session_target: SessionTarget,
    pub prompt: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub delete_after_run: bool,
    #[serde(default)]
    pub created_at_s: i64,
    pub last_output: Option<String>,
    pub channel: Option<String>,
    pub chat_id: Option<String>,
    pub session_key: Option<String>,
}

fn default_true() -> bool {
    true
}

pub struct CronScheduler {
    jobs: Arc<Mutex<HashMap<String, CronJob>>>,
    bus: Arc<Bus>,
}

use std::fs;
use std::path::PathBuf;

impl CronScheduler {
    pub fn init(_allocator: (), _config: &Config, bus: &Arc<Bus>) -> Self {
        let scheduler = Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            bus: bus.clone(),
        };
        scheduler.load();
        scheduler
    }

    pub fn load(&self) {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let mut path = PathBuf::from(home);
        path.push(".openpaw");
        path.push("cron.json");

        if let Ok(data) = fs::read_to_string(path) {
            if let Ok(jobs) = serde_json::from_str::<HashMap<String, CronJob>>(&data) {
                let mut guard = self.jobs.lock().unwrap();
                *guard = jobs;
            }
        }
    }

    pub fn save(&self) {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let mut path = PathBuf::from(home);
        path.push(".openpaw");
        path.push("cron.json");

        let guard = self.jobs.lock().unwrap();
        if let Ok(data) = serde_json::to_string_pretty(&*guard) {
            let _ = fs::write(path, data);
        }
    }

    /// Check all jobs and fire any that are due. Call this every ~60s.
    pub fn tick(&self) {
        self.load();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let mut jobs = self.jobs.lock().unwrap();
        let mut to_fire: Vec<String> = Vec::new();
        let mut to_delete: Vec<String> = Vec::new();
        let mut modified = false;

        for (id, job) in jobs.iter_mut() {
            if !job.enabled || job.paused {
                continue;
            }

            // Initialize next_run_secs if it is 0
            if job.next_run_secs == 0 {
                if let Ok(interval) = parse_duration(&job.expression) {
                    job.next_run_secs = now + interval;
                    modified = true;
                } else {
                    job.paused = true;
                    modified = true;
                }
            }

            if job.next_run_secs > 0 && now >= job.next_run_secs {
                to_fire.push(id.clone());
                job.last_run_secs = Some(now);
                job.last_status = Some("running".to_string());
                modified = true;

                // Advance next_run by parsing expression as an interval
                if let Ok(interval) = parse_duration(&job.expression) {
                    job.next_run_secs = now + interval;
                } else {
                    // One-shot or unparseable — pause it
                    job.paused = true;
                }

                if job.delete_after_run || job.one_shot {
                    to_delete.push(id.clone());
                }
            }
        }

        for id in &to_delete {
            jobs.remove(id);
            modified = true;
        }
        drop(jobs);

        if modified {
            self.save();
        }

        for id in to_fire {
            let guard = self.jobs.lock().unwrap();
            let (cmd, channel, chat_id, session_key) = if let Some(j) = guard.get(&id) {
                (
                    j.command.clone(),
                    j.channel.clone().unwrap_or_else(|| "cron".to_string()),
                    j.chat_id.clone().unwrap_or_else(|| "cron".to_string()),
                    j.session_key
                        .clone()
                        .unwrap_or_else(|| format!("cron_{}", id)),
                )
            } else {
                continue;
            };
            drop(guard);
            let _ = self.bus.publish_inbound(crate::bus::make_inbound(
                &channel,
                &channel,
                &chat_id,
                &cmd,
                &session_key,
            ));
        }
    }

    pub fn add_job(&self, job: CronJob) {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.insert(job.id.clone(), job);
    }

    pub fn remove_job(&self, id: &str) -> Option<CronJob> {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.remove(id)
    }

    pub fn list_jobs(&self) -> Vec<CronJob> {
        let jobs = self.jobs.lock().unwrap();
        jobs.values().cloned().collect()
    }
}

pub fn parse_duration(input: &str) -> Result<i64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Empty delay string"));
    }

    let last_char = trimmed.chars().last().unwrap();
    let (num_str, multiplier) = if last_char.is_ascii_alphabetic() {
        let num_part = &trimmed[..trimmed.len() - 1];
        let mult = match last_char {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86400,
            'w' => 604800,
            _ => return Err(anyhow!("Unknown duration unit: {}", last_char)),
        };
        (num_part, mult)
    } else {
        (trimmed, 60) // Default to minutes
    };

    let n: i64 = num_str.trim().parse()?;
    if n <= 0 {
        return Err(anyhow!("Invalid duration number: {}", n));
    }

    Ok(n * multiplier)
}
