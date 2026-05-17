/// Curator — background skill maintenance orchestrator (Hermes-style).
///
/// The curator is a background system that periodically reviews agent-created
/// skills and maintains the skill collection. It runs using the cheap/auxiliary
/// model (not the main session), triggered by idle detection and configured
/// interval.
///
/// Responsibilities:
///   - Auto-transition lifecycle states based on skill activity timestamps
///   - Spawn a background review that consolidates, archives, and patches skills
///   - Persist curator state (last_run_at, paused, etc.)
///   - Never auto-deletes — only archives
///   - Pinned skills bypass all auto-transitions
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::info;

use crate::skills::usage::{self, ActivityKind, SkillUsageDB, STATE_ACTIVE, STATE_ARCHIVED, STATE_STALE};

// ── Default constants ─────────────────────────────────────────────

pub const DEFAULT_INTERVAL_HOURS: u64 = 24 * 7; // 7 days
pub const DEFAULT_MIN_IDLE_HOURS: f64 = 2.0;
pub const DEFAULT_STALE_AFTER_DAYS: i64 = 30;
pub const DEFAULT_ARCHIVE_AFTER_DAYS: i64 = 90;

// ── Curator state persistence ─────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratorState {
    pub last_run_at: Option<String>,
    pub last_run_duration_seconds: Option<f64>,
    pub last_run_summary: Option<String>,
    pub last_report_path: Option<String>,
    pub paused: bool,
    pub run_count: u64,
}

impl Default for CuratorState {
    fn default() -> Self {
        Self {
            last_run_at: None,
            last_run_duration_seconds: None,
            last_run_summary: None,
            last_report_path: None,
            paused: false,
            run_count: 0,
        }
    }
}

fn curator_state_path(workspace_dir: &str) -> PathBuf {
    Path::new(workspace_dir).join("state").join(".curator_state.json")
}

fn load_state(workspace_dir: &str) -> CuratorState {
    let path = curator_state_path(workspace_dir);
    if path.exists() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(state) = serde_json::from_str::<CuratorState>(&content) {
                return state;
            }
        }
    }
    CuratorState::default()
}

fn save_state(workspace_dir: &str, state: &CuratorState) {
    let path = curator_state_path(workspace_dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let tmp = path.with_extension("tmp");
        let _ = std::fs::write(&tmp, &json);
        let _ = std::fs::rename(&tmp, &path);
    }
}

// ── Config ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CuratorConfig {
    /// Whether the curator is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Hours between curator runs.
    #[serde(default = "default_interval_hours")]
    pub interval_hours: u64,
    /// Minimum hours of idle time before running.
    #[serde(default = "default_min_idle_hours")]
    pub min_idle_hours: f64,
    /// Days of inactivity before a skill is marked stale.
    #[serde(default = "default_stale_after_days")]
    pub stale_after_days: i64,
    /// Days of inactivity before a stale skill is archived.
    #[serde(default = "default_archive_after_days")]
    pub archive_after_days: i64,
}

impl Default for CuratorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_hours: DEFAULT_INTERVAL_HOURS,
            min_idle_hours: DEFAULT_MIN_IDLE_HOURS,
            stale_after_days: DEFAULT_STALE_AFTER_DAYS,
            archive_after_days: DEFAULT_ARCHIVE_AFTER_DAYS,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_interval_hours() -> u64 {
    DEFAULT_INTERVAL_HOURS
}
fn default_min_idle_hours() -> f64 {
    DEFAULT_MIN_IDLE_HOURS
}
fn default_stale_after_days() -> i64 {
    DEFAULT_STALE_AFTER_DAYS
}
fn default_archive_after_days() -> i64 {
    DEFAULT_ARCHIVE_AFTER_DAYS
}

// ── Should-run gate ───────────────────────────────────────────────

/// Check whether the curator should run now based on interval and paused state.
/// First-run: seeds the state so the curator waits a full interval before first run.
pub fn should_run_now(workspace_dir: &str, config: &CuratorConfig) -> bool {
    if !config.enabled {
        return false;
    }

    let state = load_state(workspace_dir);
    if state.paused {
        return false;
    }

    let now = now_secs();

    match &state.last_run_at {
        None => {
            // First time — seed state so we wait a full interval
            let new_state = CuratorState {
                last_run_at: Some(iso_now()),
                last_run_summary: Some(
                    "deferred first run — curator seeded, will run after one interval".to_string(),
                ),
                ..state
            };
            save_state(workspace_dir, &new_state);
            false
        }
        Some(ts) => {
            let last = parse_iso(ts).unwrap_or(0);
            let interval = Duration::from_secs(config.interval_hours * 3600);
            now.saturating_sub(last) >= interval.as_secs()
        }
    }
}

// ── Automatic state transitions (pure function, no LLM) ───────────

/// Walk every agent-created skill and transition active→stale→archived
/// based on activity timestamps. Pinned skills are never touched.
/// Returns counts of what changed.
#[derive(Debug, Default)]
pub struct AutoTransitionCounts {
    pub checked: u32,
    pub marked_stale: u32,
    pub archived: u32,
    pub reactivated: u32,
}

pub fn apply_automatic_transitions(
    usage_db: &SkillUsageDB,
    config: &CuratorConfig,
) -> Result<AutoTransitionCounts> {
    let reports = usage_db.agent_created_report()?;
    let now_secs = now_secs() as i64;
    let stale_cutoff = now_secs - config.stale_after_days * 86400;
    let archive_cutoff = now_secs - config.archive_after_days * 86400;

    let mut counts = AutoTransitionCounts::default();

    for row in &reports {
        counts.checked += 1;
        if row.pinned {
            continue;
        }

        // Anchor: last_activity_at in unix seconds, or created_at, or now
        let anchor = row
            .last_activity_at
            .as_deref()
            .and_then(parse_iso)
            .or_else(|| row.created_at.as_deref().and_then(parse_iso))
            .unwrap_or(now_secs as u64) as i64;

        let current = row.state.as_str();

        if anchor <= archive_cutoff && current != STATE_ARCHIVED {
            usage_db.set_state(&row.name, STATE_ARCHIVED)?;
            usage_db.record(&row.name, ActivityKind::Archive, "auto-transition: stale time expired")?;
            counts.archived += 1;
            info!("curator auto: archived skill '{}' (inactive since {})", row.name, row.last_activity_at.as_deref().unwrap_or("never"));
        } else if anchor <= stale_cutoff && current == STATE_ACTIVE {
            usage_db.set_state(&row.name, STATE_STALE)?;
            usage_db.record(&row.name, ActivityKind::MarkStale, "auto-transition: exceeded stale threshold")?;
            counts.marked_stale += 1;
            info!("curator auto: marked skill '{}' as stale", row.name);
        } else if anchor > stale_cutoff && current == STATE_STALE {
            // Reactivate: skill used again after being marked stale
            usage_db.set_state(&row.name, STATE_ACTIVE)?;
            usage_db.record(&row.name, ActivityKind::Reactivate, "auto-transition: recent activity")?;
            counts.reactivated += 1;
            info!("curator auto: reactivated skill '{}'", row.name);
        }
    }

    Ok(counts)
}

// ── Candidate list rendering ──────────────────────────────────────

/// Render a candidate list of agent-created skills for the curator LLM to review.
pub fn render_candidate_list(usage_db: &SkillUsageDB) -> Result<String> {
    let reports = usage_db.agent_created_report()?;
    if reports.is_empty() {
        return Ok("No agent-created skills to review.".to_string());
    }

    let mut lines = vec![format!("Agent-created skills ({}):\n", reports.len())];
    for row in &reports {
        lines.push(usage::format_skill_row(row));
    }
    Ok(lines.join("\n"))
}

// ── Curator review prompt (condensed from Hermes) ─────────────────

pub const CURATOR_REVIEW_PROMPT: &str = r#"You are running as OpenPaw's background skill CURATOR. This is an UMBRELLA-BUILDING consolidation pass.

The goal: maintain a library of class-level instructions and experiential knowledge. Many narrow skills each capturing one session's specific bug is a failure — not a feature. One broad umbrella skill with labeled subsections beats five narrow siblings for discoverability.

Hard rules:
1. DO NOT touch bundled or hub-installed skills — only agent-created ones.
2. DO NOT delete any skill. Archiving (moving to .archive/) is the maximum destructive action.
3. DO NOT touch pinned skills. Skip them entirely.
4. Judge overlap on CONTENT, not on use_count. use=0 is not evidence a skill is valuable.
5. 'keep' is legitimate ONLY when the skill is already a class-level umbrella.

How to work:
1. Scan the full candidate list. Identify PREFIX CLUSTERS (skills sharing a domain keyword).
2. For each cluster with 2+ members: ask "what UMBRELLA CLASS do these serve?"
3. Consolidate via:
   a. MERGE INTO EXISTING — if one is broad enough, patch it and archive siblings
   b. CREATE NEW UMBRELLA — use skill_manage action=create
   c. DEMOTE TO REFERENCES — move narrow but valuable content into umbrella's references/
4. Flag skills whose NAME is too narrow (contains a PR number, error string, etc.)

When done, summarize what you did. If you archived skills, list them with reasons."#;

// ── Main curator pass (spawned as background task) ─────────────

/// Run a full curator pass: auto-transitions + LLM review via cheap provider.
/// Returns a human-readable summary of what happened.
pub async fn run_curator_pass(
    usage_db: &crate::skills::usage::SkillUsageDB,
    cheap_provider: &Arc<dyn crate::providers::Provider>,
    cheap_model: &str,
    workspace_dir: &str,
) -> Result<String> {
    let start = std::time::Instant::now();
    let config = CuratorConfig::default();

    // Step 1: Apply automatic state transitions (no LLM)
    let auto_counts = apply_automatic_transitions(usage_db, &config)?;

    let mut auto_summary_parts = Vec::new();
    if auto_counts.marked_stale > 0 {
        auto_summary_parts.push(format!("{} marked stale", auto_counts.marked_stale));
    }
    if auto_counts.archived > 0 {
        auto_summary_parts.push(format!("{} archived", auto_counts.archived));
    }
    if auto_counts.reactivated > 0 {
        auto_summary_parts.push(format!("{} reactivated", auto_counts.reactivated));
    }
    let auto_summary = if auto_summary_parts.is_empty() {
        "no changes".to_string()
    } else {
        auto_summary_parts.join(", ")
    };

    // Step 2: Build candidate list for LLM review
    let candidate_list = match render_candidate_list(usage_db) {
        Ok(cl) => cl,
        Err(e) => {
            // Record the partial run
            let summary = format!("auto: {}; llm: skipped (error: {})", auto_summary, e);
            record_run(workspace_dir, start.elapsed().as_secs_f64(), &summary, None);
            return Ok(summary);
        }
    };

    if candidate_list.contains("No agent-created skills") {
        let summary = format!("auto: {}; llm: skipped (no candidates)", auto_summary);
        record_run(workspace_dir, start.elapsed().as_secs_f64(), &summary, None);
        return Ok(summary);
    }

    // Step 3: Run LLM review
    let prompt = format!("{}\n\n{}", CURATOR_REVIEW_PROMPT, candidate_list);

    let request = crate::providers::ChatRequest {
        messages: &[crate::providers::ChatMessage::user(prompt)],
        model: cheap_model,
        temperature: 0.3,
        max_tokens: Some(2000),
        tools: None, // Curator doesn't need tools — it's analysis-only for now
        timeout_secs: 120,
        reasoning_effort: None,
    };

    let llm_summary = match cheap_provider.chat(&request) {
        Ok(resp) => resp.content.unwrap_or_else(|| "no response".to_string()),
        Err(e) => {
            let summary = format!("auto: {}; llm: error ({})", auto_summary, e);
            record_run(workspace_dir, start.elapsed().as_secs_f64(), &summary, None);
            return Ok(summary);
        }
    };

    // Step 4: Write report
    let elapsed = start.elapsed().as_secs_f64();
    let summary = format!("auto: {}; llm: reviewed", auto_summary);

    let _report_path = write_run_report(
        workspace_dir,
        &auto_counts,
        &candidate_list,
        &llm_summary,
        cheap_model,
    );

    record_run(
        workspace_dir,
        elapsed,
        &summary,
        _report_path.as_ref().ok().and_then(|p| p.to_str()),
    );

    Ok(summary)
}

// ── Helpers ────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn iso_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn parse_iso(ts: &str) -> Option<u64> {
    // Try to parse ISO 8601 or SQLite datetime strings
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        return Some(dt.timestamp() as u64);
    }
    // Try "YYYY-MM-DD HH:MM:SS" (SQLite format)
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S") {
        return Some(dt.and_utc().timestamp() as u64);
    }
    None
}

/// Update curator state after a run.
pub fn record_run(
    workspace_dir: &str,
    duration_seconds: f64,
    summary: &str,
    report_path: Option<&str>,
) {
    let mut state = load_state(workspace_dir);
    state.last_run_at = Some(iso_now());
    state.last_run_duration_seconds = Some(duration_seconds);
    state.last_run_summary = Some(summary.to_string());
    state.last_report_path = report_path.map(|s| s.to_string());
    state.run_count += 1;
    save_state(workspace_dir, &state);
}

/// Write a curator run report to the workspace.
pub fn write_run_report(
    workspace_dir: &str,
    auto_counts: &AutoTransitionCounts,
    candidate_report: &str,
    llm_summary: &str,
    model_used: &str,
) -> Result<PathBuf> {
    let reports_dir = Path::new(workspace_dir).join("state").join("curator_reports");
    std::fs::create_dir_all(&reports_dir)?;

    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let run_dir = reports_dir.join(stamp.to_string());
    std::fs::create_dir_all(&run_dir)?;

    let report = format!(
        "# Curator Run — {stamp}\n\n\
         Model: `{model}`\n\n\
         ## Auto-transitions (no LLM)\n\
         - checked: {checked}\n\
         - marked stale: {stale}\n\
         - archived: {archived}\n\
         - reactivated: {reactivated}\n\n\
         ## Candidate Skills Reviewed\n\
         {candidates}\n\n\
         ## LLM Summary\n\
         {llm}\n",
        stamp = stamp,
        model = model_used,
        checked = auto_counts.checked,
        stale = auto_counts.marked_stale,
        archived = auto_counts.archived,
        reactivated = auto_counts.reactivated,
        candidates = candidate_report,
        llm = llm_summary,
    );

    let report_path = run_dir.join("REPORT.md");
    std::fs::write(&report_path, report)?;

    // Also write machine-readable summary
    let json_path = run_dir.join("run.json");
    let json = serde_json::json!({
        "stamp": stamp.to_string(),
        "model": model_used,
        "auto_transitions": {
            "checked": auto_counts.checked,
            "marked_stale": auto_counts.marked_stale,
            "archived": auto_counts.archived,
            "reactivated": auto_counts.reactivated,
        },
        "llm_summary": llm_summary,
    });
    std::fs::write(&json_path, serde_json::to_string_pretty(&json)?)?;

    Ok(run_dir)
}
