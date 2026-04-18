use crate::bus::{Bus, InboundMessage, OutboundMessage};
use crate::config::Config;
use crate::streaming::OutboundStage;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Cross-platform path to the OpenPaw data directory (~/.openpaw).
/// Uses HOME on Linux/macOS, USERPROFILE on Windows, falls back to "."
fn openpaw_data_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".openpaw")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum JobType {
    #[default]
    Shell,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SessionTarget {
    #[default]
    Isolated,
    Main,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleKind {
    Cron,
    At,
    Every,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Schedule {
    Cron {
        expression: String,
        tz: Option<String>,
    },
    At {
        timestamp_s: i64,
    },
    Every {
        every_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum DeliveryMode {
    #[default]
    None,
    Always,
    OnError,
    OnSuccess,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeliveryConfig {
    #[serde(default)]
    pub mode: DeliveryMode,
    pub channel: Option<String>,
    pub account_id: Option<String>,
    pub to: Option<String>,
    #[serde(default = "default_true")]
    pub best_effort: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronRun {
    pub id: u64,
    pub job_id: String,
    pub started_at_s: i64,
    pub finished_at_s: i64,
    pub status: String,
    pub output: Option<String>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub expression: String, // Kept for backward compatibility or as the primary source
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
    #[serde(default)]
    pub delivery: DeliveryConfig,
    #[serde(skip)]
    pub history: Vec<CronRun>,
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

fn default_true() -> bool {
    true
}

fn default_timezone() -> String {
    "UTC".to_string()
}

pub struct CronScheduler {
    pub jobs: Arc<Mutex<HashMap<String, CronJob>>>,
    pub runs: Arc<Mutex<Vec<CronRun>>>,
    pub bus: Arc<Bus>,
    pub next_run_id: Arc<Mutex<u64>>,
    /// Tracks IDs of jobs currently executing to prevent concurrent duplicate runs.
    running_jobs: Arc<Mutex<HashSet<String>>>,
}

use std::fs;

impl CronScheduler {
    pub fn init(_allocator: (), _config: &Config, bus: &Arc<Bus>) -> Self {
        let scheduler = Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            runs: Arc::new(Mutex::new(Vec::new())),
            bus: bus.clone(),
            next_run_id: Arc::new(Mutex::new(1)),
            running_jobs: Arc::new(Mutex::new(HashSet::new())),
        };
        scheduler.load();
        scheduler
    }

    pub fn load(&self) {
        let mut path = openpaw_data_dir();
        path.push("cron.json");

        if let Ok(data) = fs::read_to_string(path)
            && let Ok(jobs) = serde_json::from_str::<HashMap<String, CronJob>>(&data)
        {
            let mut guard = self.jobs.lock().unwrap();
            *guard = jobs;
        }
    }

    pub fn save(&self) {
        let mut path = openpaw_data_dir();
        path.push("cron.json");

        // Ensure the directory exists
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let guard = self.jobs.lock().unwrap();
        if let Ok(data) = serde_json::to_string_pretty(&*guard) {
            let tmp_path = path.with_extension("tmp");
            if let Ok(mut file) = fs::File::create(&tmp_path) {
                use std::io::Write;
                if file.write_all(data.as_bytes()).is_ok() && file.sync_all().is_ok() {
                    let _ = fs::rename(&tmp_path, path);
                } else {
                    let _ = fs::remove_file(&tmp_path);
                }
            }
        }
    }

    pub fn tick(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let mut jobs = self.jobs.lock().unwrap();
        let mut to_fire: Vec<(String, Option<CronJob>)> = Vec::new();
        let mut to_delete: Vec<String> = Vec::new();
        let mut modified = false;

        for (id, job) in jobs.iter_mut() {
            if !job.enabled || job.paused {
                continue;
            }

            // Initialize next_run_secs if it is 0
            if job.next_run_secs == 0 {
                if let Ok(next) = next_run_for_expression(&job.expression, now, &job.timezone) {
                    job.next_run_secs = next;
                    modified = true;
                } else {
                    job.paused = true;
                    modified = true;
                }
            }

            if job.next_run_secs > 0 && now >= job.next_run_secs {
                let is_ephemeral = job.delete_after_run || job.one_shot;
                let snapshot = if is_ephemeral { Some(job.clone()) } else { None };
                to_fire.push((id.clone(), snapshot));
                job.last_run_secs = Some(now);
                job.last_status = Some("running".to_string());
                modified = true;

                // Advance next_run
                if let Ok(next) = next_run_for_expression(&job.expression, now, &job.timezone) {
                    job.next_run_secs = next;
                } else {
                    job.paused = true;
                }

                if is_ephemeral {
                    to_delete.push(id.clone());
                }
            }
        }

        for id in &to_delete {
            jobs.remove(id);
            modified = true;
        }

        if modified {
            drop(jobs);
            self.save();
        } else {
            drop(jobs);
        }

        for (id, snapshot) in to_fire {
            // Skip jobs that are already running to prevent concurrent duplicate runs.
            {
                let mut running = self.running_jobs.lock().unwrap();
                if !running.insert(id.clone()) {
                    continue;
                }
            }
            let scheduler = self.clone_with_arcs();
            tokio::spawn(async move {
                let _ = scheduler.run_job_with_snapshot(&id, snapshot).await;
            });
        }
    }

    fn clone_with_arcs(&self) -> Self {
        Self {
            jobs: self.jobs.clone(),
            runs: self.runs.clone(),
            bus: self.bus.clone(),
            next_run_id: self.next_run_id.clone(),
            running_jobs: self.running_jobs.clone(),
        }
    }

    pub async fn run_job(&self, id: &str) -> Result<()> {
        self.run_job_with_snapshot(id, None).await
    }

    pub async fn run_job_with_snapshot(&self, id: &str, snapshot: Option<CronJob>) -> Result<()> {
        let id_str = id.to_string();
        let job = match snapshot {
            Some(j) => j,
            None => {
                let guard = self.jobs.lock().unwrap();
                match guard.get(id) {
                    Some(j) => j.clone(),
                    None => return Err(anyhow!("Job not found")),
                }
            }
        };

        // Mark as running; bail if already in progress.
        {
            let mut running = self.running_jobs.lock().unwrap();
            if !running.insert(id_str.clone()) {
                return Err(anyhow!("Job {} is already running", id_str));
            }
        }

        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let result = match job.job_type {
            JobType::Shell => {
                // Cross-platform shell execution
                let output = if cfg!(windows) {
                    tokio::process::Command::new("cmd")
                        .arg("/c")
                        .arg(&job.command)
                        .output()
                        .await
                } else {
                    tokio::process::Command::new("sh")
                        .arg("-c")
                        .arg(&job.command)
                        .output()
                        .await
                };

                match output {
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                        let combined = if stderr.is_empty() {
                            stdout
                        } else {
                            format!("{}\n{}", stdout, stderr)
                        };
                        (out.status.success(), combined)
                    }
                    Err(e) => (false, format!("Failed to execute shell command: {}", e)),
                }
            }
            JobType::Agent => {
                let channel = job
                    .delivery
                    .channel
                    .clone()
                    .unwrap_or_else(|| "internal".to_string());
                let chat_id = job
                    .delivery
                    .to
                    .clone()
                    .unwrap_or_else(|| "cron".to_string());
                let content = job.prompt.as_deref().unwrap_or(&job.command).to_string();
                let session_key = format!("{channel}:{chat_id}");

                let inbound = InboundMessage {
                    channel,
                    sender_id: "cron".to_string(),
                    chat_id: chat_id.clone(),
                    content,
                    session_key,
                    media: Vec::new(),
                    metadata_json: None,
                    task_kind: Some("cron".to_string()),
                    is_group: false,
                };
                match self.bus.publish_inbound(inbound) {
                    Ok(()) => (
                        true,
                        format!(
                            "Agent job '{}' dispatched to session",
                            job.id
                        ),
                    ),
                    Err(e) => (
                        false,
                        format!("Failed to dispatch agent job via bus: {}", e),
                    ),
                }
            }
        };

        let (success, output) = result;

        let finished_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // Update last output and status
        {
            let mut guard = self.jobs.lock().unwrap();
            if let Some(j) = guard.get_mut(id) {
                j.last_status = Some(if success {
                    "ok".to_string()
                } else {
                    "error".to_string()
                });
                j.last_output = Some(output.clone());
                j.last_run_secs = Some(finished_at);
            }
        }

        // Add to history
        {
            let mut run_id_guard = self.next_run_id.lock().unwrap();
            let run_id = *run_id_guard;
            *run_id_guard += 1;

            let run = CronRun {
                id: run_id,
                job_id: id_str.clone(),
                started_at_s: started_at,
                finished_at_s: finished_at,
                status: if success {
                    "ok".to_string()
                } else {
                    "error".to_string()
                },
                output: Some(output.clone()),
                duration_ms: Some((finished_at - started_at) * 1000),
            };

            let mut runs_guard = self.runs.lock().unwrap();
            runs_guard.push(run);
            // Prune history (keep last 50 per job for now)
            if runs_guard.len() > 1000 {
                runs_guard.remove(0);
            }
        }

        // For Agent jobs, skip deliver_result: the agent's own response is routed
        // through the inbound/outbound bus pipeline. Sending the dispatch
        // confirmation string here would leak internal diagnostics to the user's chat.
        if job.job_type != JobType::Agent {
            let _ = Self::deliver_result(&self.bus, &job.delivery, &output, success).await;
        }

        // Deregister from running set.
        self.running_jobs.lock().unwrap().remove(&id_str);

        Ok(())
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

    async fn deliver_result(
        bus: &Arc<Bus>,
        delivery: &DeliveryConfig,
        content: &str,
        success: bool,
    ) -> Result<()> {
        if delivery.mode == DeliveryMode::None {
            return Ok(());
        }

        match delivery.mode {
            DeliveryMode::OnSuccess if !success => return Ok(()),
            DeliveryMode::OnError if success => return Ok(()),
            _ => {}
        }

        if content.is_empty() {
            return Ok(());
        }

        let channel = match &delivery.channel {
            Some(c) => c,
            None => return Ok(()),
        };

        let chat_id = delivery.to.clone().unwrap_or_else(|| "default".to_string());

        let msg = OutboundMessage {
            channel: channel.clone(),
            account_id: delivery.account_id.clone(),
            chat_id,
            content: content.to_string(),
            media: Vec::new(),
            stage: OutboundStage::Final,
        };

        let _ = bus.publish_outbound(msg);
        Ok(())
    }
}

pub struct CronNormalized {
    pub expression: String,
    pub needs_second_prefix: bool,
}

pub fn normalize_expression(expression: &str) -> Result<CronNormalized> {
    let trimmed = expression.trim();
    let fields: Vec<&str> = trimmed.split_whitespace().collect();

    match fields.len() {
        5 => Ok(CronNormalized {
            expression: trimmed.to_string(),
            needs_second_prefix: true,
        }),
        6 | 7 => Ok(CronNormalized {
            expression: trimmed.to_string(),
            needs_second_prefix: false,
        }),
        _ => Err(anyhow!(
            "Invalid cron expression: expected 5, 6, or 7 fields"
        )),
    }
}

// Old cron parsers removed

pub fn next_run_for_expression(expression: &str, from_secs: i64, timezone: &str) -> Result<i64> {
    if let Some(delay) = expression.strip_prefix("@once:") {
        let delay_secs = parse_duration(delay)?;
        return Ok(from_secs + delay_secs); // Delays are timezone-agnostic (relative)
    }

    use chrono::{TimeZone, Utc};
    use chrono_tz::Tz;
    use std::str::FromStr;

    let normalized = normalize_expression(expression)?;
    
    // We must pass a 7-part expression to `cron` crate.
    let parts: Vec<&str> = normalized.expression.split_whitespace().collect();
    let cron_str = if normalized.needs_second_prefix {
        // Was 5 fields, now 6 with 0 at the start. Need 7 for `cron` crate (add year `*`).
        if parts.len() == 5 {
            format!("0 {} *", normalized.expression)
        } else {
            normalized.expression.clone()
        }
    } else {
        if parts.len() == 6 {
            format!("{} *", normalized.expression)
        } else {
            normalized.expression.clone()
        }
    };

    let schedule = cron::Schedule::from_str(&cron_str)
        .map_err(|e| anyhow!("Failed to parse cron expression: {}", e))?;

    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    
    let dt = tz
        .timestamp_opt(from_secs, 0)
        .single()
        .unwrap_or_else(|| Utc::now().with_timezone(&tz));

    if let Some(next) = schedule.after(&dt).next() {
        Ok(next.timestamp())
    } else {
        Err(anyhow!("No future run found within schedule limits"))
    }
}

pub fn parse_duration(input: &str) -> Result<i64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Empty duration string"));
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
