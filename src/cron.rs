use crate::bus::Bus;
use crate::config::Config;
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
}

fn default_true() -> bool {
    true
}

pub struct CronScheduler {
    jobs: Arc<Mutex<HashMap<String, CronJob>>>,
    bus: Arc<Bus>,
}

impl CronScheduler {
    pub fn init(_allocator: (), _config: &Config, bus: &Arc<Bus>) -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            bus: bus.clone(),
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
