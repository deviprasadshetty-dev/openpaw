use crate::bus::{Bus, InboundMessage, OutboundMessage};
use crate::config::Config;
use crate::streaming::OutboundStage;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Send a desktop notification (cross-platform: Windows/macOS/Linux)
fn send_desktop_notification(title: &str, body: &str) {
    #[cfg(not(target_os = "android"))]
    {
        let _ = notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .icon("dialog-information")
            .show();
    }
    // On Android (if ever supported), we would use a different mechanism
}

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
            let _ = fs::write(path, data);
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
                let running = self.running_jobs.lock().unwrap();
                if running.contains(&id) {
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

        let (success, output) = match job.job_type {
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
                // BUG-CRON-2 FIX: Route through the live in-process bus instead of spawning
                // a subprocess. This ensures the agent job has access to the existing
                // session, memory, and all registered tools.
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
                };
                match self.bus.publish_inbound(inbound) {
                    Ok(()) => (
                        true,
                        format!(
                            "Agent job '{}' dispatched via bus to chat {}",
                            job.id, chat_id
                        ),
                    ),
                    Err(e) => (
                        false,
                        format!("Failed to dispatch agent job via bus: {}", e),
                    ),
                }
            }
        };

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

        // Deliver result
        let _ = Self::deliver_result(&self.bus, &job.delivery, &output, success).await;

        // Send desktop notification for reminders/agent jobs
        if job.job_type == JobType::Agent || !job.command.is_empty() {
            let title = if success {
                "✅ Reminder Completed"
            } else {
                "⚠️ Reminder Failed"
            };
            let body = if job.name.is_some() {
                format!(
                    "{}: {}",
                    job.name.as_ref().unwrap(),
                    output.chars().take(100).collect::<String>()
                )
            } else {
                output.chars().take(100).collect::<String>()
            };
            send_desktop_notification(title, &body);
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

pub struct ParsedCronExpression {
    pub minutes: [bool; 60],
    pub hours: [bool; 24],
    pub day_of_month: [bool; 32],
    pub months: [bool; 13],
    pub day_of_week: [bool; 7],
    pub day_of_month_any: bool,
    pub day_of_week_any: bool,
}

fn parse_cron_field(
    field: &str,
    min: u8,
    max: u8,
    allow_sunday_7: bool,
    out: &mut [bool],
) -> Result<bool> {
    for val in out.iter_mut() {
        *val = false;
    }

    if field == "*" {
        for i in min..=max {
            out[i as usize] = true;
        }
        return Ok(true);
    }

    let mut saw_value = false;
    for part in field.split(',') {
        let (range_part, step) = if let Some(slash_idx) = part.find('/') {
            let step_str = &part[slash_idx + 1..];
            let step: u8 = step_str.parse().map_err(|_| anyhow!("Invalid step"))?;
            (&part[..slash_idx], step)
        } else {
            (part, 1)
        };

        let (start, end) = if range_part == "*" {
            (min, max)
        } else if let Some(dash_idx) = range_part.find('-') {
            let start_str = &range_part[..dash_idx];
            let end_str = &range_part[dash_idx + 1..];
            let start: u8 = start_str
                .parse()
                .map_err(|_| anyhow!("Invalid range start"))?;
            let end: u8 = end_str.parse().map_err(|_| anyhow!("Invalid range end"))?;
            (start, end)
        } else {
            let val: u8 = range_part
                .parse()
                .map_err(|_| anyhow!("Invalid cron value"))?;
            (val, val)
        };

        if start < min || end > (if allow_sunday_7 { 7 } else { max }) || start > end {
            return Err(anyhow!("Cron value out of range"));
        }

        let mut current = start;
        while current <= end {
            let normalized = if allow_sunday_7 && current == 7 {
                0
            } else {
                current
            };
            if normalized >= min && (normalized as usize) < out.len() {
                out[normalized as usize] = true;
                saw_value = true;
            }
            if let Some(next) = current.checked_add(step) {
                current = next;
            } else {
                break;
            }
        }
    }

    if !saw_value {
        return Err(anyhow!("Invalid cron field: {}", field));
    }
    Ok(false)
}

pub fn parse_cron_expression(expression: &str) -> Result<ParsedCronExpression> {
    let normalized = normalize_expression(expression)?;
    let fields: Vec<&str> = normalized.expression.split_whitespace().collect();

    let (min_f, hour_f, dom_f, mon_f, dow_f) = if normalized.needs_second_prefix {
        (fields[0], fields[1], fields[2], fields[3], fields[4])
    } else {
        (fields[1], fields[2], fields[3], fields[4], fields[5])
    };

    let mut parsed = ParsedCronExpression {
        minutes: [false; 60],
        hours: [false; 24],
        day_of_month: [false; 32],
        months: [false; 13],
        day_of_week: [false; 7],
        day_of_month_any: false,
        day_of_week_any: false,
    };

    parse_cron_field(min_f, 0, 59, false, &mut parsed.minutes)?;
    parse_cron_field(hour_f, 0, 23, false, &mut parsed.hours)?;
    parsed.day_of_month_any = parse_cron_field(dom_f, 1, 31, false, &mut parsed.day_of_month)?;
    parse_cron_field(mon_f, 1, 12, false, &mut parsed.months)?;
    parsed.day_of_week_any = parse_cron_field(dow_f, 0, 6, true, &mut parsed.day_of_week)?;

    Ok(parsed)
}

fn cron_matches(parsed: &ParsedCronExpression, ts: i64, timezone: &str) -> bool {
    use chrono::{Datelike, TimeZone, Timelike};
    use chrono_tz::Tz;

    // Parse timezone string, fallback to UTC if invalid
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let dt = tz
        .timestamp_opt(ts, 0)
        .single()
        .unwrap_or_else(|| chrono::Utc::now().with_timezone(&tz));

    let minute = dt.minute() as usize;
    let hour = dt.hour() as usize;
    let day = dt.day() as usize;
    let month = dt.month() as usize;
    let dow = dt.weekday().num_days_from_sunday() as usize;

    if minute >= 60 || !parsed.minutes[minute] {
        return false;
    }
    if hour >= 24 || !parsed.hours[hour] {
        return false;
    }
    if month > 12 || !parsed.months[month] {
        return false;
    }

    let dom_match = day <= 31 && parsed.day_of_month[day];
    let dow_match = dow < 7 && parsed.day_of_week[dow];

    if parsed.day_of_month_any && parsed.day_of_week_any {
        true
    } else if parsed.day_of_month_any {
        dow_match
    } else if parsed.day_of_week_any {
        dom_match
    } else {
        dom_match || dow_match
    }
}

pub fn next_run_for_expression(expression: &str, from_secs: i64, timezone: &str) -> Result<i64> {
    if let Some(delay) = expression.strip_prefix("@once:") {
        let delay_secs = parse_duration(delay)?;
        return Ok(from_secs + delay_secs); // Delays are timezone-agnostic (relative)
    }

    let parsed = parse_cron_expression(expression)?;
    let mut candidate = from_secs - (from_secs % 60) + 60;

    // Look ahead 1 year
    for _ in 0..(366 * 24 * 60) {
        if cron_matches(&parsed, candidate, timezone) {
            return Ok(candidate);
        }
        candidate += 60;
    }

    Err(anyhow!("No future run found within 1 year"))
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
