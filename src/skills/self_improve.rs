/// Skill self-improvement system — Hermes-style learning loop.
/// 
/// This module enables skills to improve themselves during use by:
/// 1. Tracking execution success/failure per skill
/// 2. Capturing user feedback (implicit via corrections, explicit via ratings)
/// 3. Periodically rewriting skill definitions based on accumulated learnings
/// 4. Maintaining a "skill journal" of executions for pattern recognition

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// A record of a single skill execution.
#[derive(Debug, Clone)]
pub struct SkillExecution {
    pub skill_name: String,
    pub tool_name: String,
    pub timestamp_secs: u64,
    pub success: bool,
    pub user_correction: Option<String>,
    pub execution_time_ms: u64,
    pub notes: String,
}

/// In-memory journal of skill executions (per session).
/// Flushed to disk periodically.
pub struct SkillJournal {
    entries: Mutex<Vec<SkillExecution>>,
    workspace_dir: String,
}

impl SkillJournal {
    pub fn new(workspace_dir: String) -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            workspace_dir,
        }
    }

    pub fn record(&self, execution: SkillExecution) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.push(execution);
    }

    pub fn record_success(&self, skill_name: &str, tool_name: &str, notes: &str) {
        self.record(SkillExecution {
            skill_name: skill_name.to_string(),
            tool_name: tool_name.to_string(),
            timestamp_secs: now_secs(),
            success: true,
            user_correction: None,
            execution_time_ms: 0,
            notes: notes.to_string(),
        });
    }

    pub fn record_failure(&self, skill_name: &str, tool_name: &str, error: &str) {
        self.record(SkillExecution {
            skill_name: skill_name.to_string(),
            tool_name: tool_name.to_string(),
            timestamp_secs: now_secs(),
            success: false,
            user_correction: None,
            execution_time_ms: 0,
            notes: error.to_string(),
        });
    }

    pub fn record_correction(&self, skill_name: &str, tool_name: &str, correction: &str) {
        self.record(SkillExecution {
            skill_name: skill_name.to_string(),
            tool_name: tool_name.to_string(),
            timestamp_secs: now_secs(),
            success: true,
            user_correction: Some(correction.to_string()),
            execution_time_ms: 0,
            notes: "User provided correction".to_string(),
        });
    }

    /// Get statistics for a specific skill.
    pub fn get_skill_stats(&self, skill_name: &str) -> (usize, usize, Vec<String>) {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let skill_entries: Vec<&SkillExecution> = entries
            .iter()
            .filter(|e| e.skill_name == skill_name)
            .collect();

        let total = skill_entries.len();
        let successes = skill_entries.iter().filter(|e| e.success).count();
        let corrections: Vec<String> = skill_entries
            .iter()
            .filter_map(|e| e.user_correction.clone())
            .collect();

        (total, successes, corrections)
    }

    /// Check if a skill has accumulated enough learnings to warrant an update.
    pub fn should_improve(&self, skill_name: &str, min_executions: usize) -> bool {
        let (total, successes, corrections) = self.get_skill_stats(skill_name);
        total >= min_executions && (successes < total || !corrections.is_empty())
    }

    /// Flush the journal to a JSONL file for persistence.
    pub fn flush(&self) -> anyhow::Result<()> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.is_empty() {
            return Ok(());
        }

        let journal_path = Path::new(&self.workspace_dir).join("state").join("skill_journal.jsonl");
        if let Some(parent) = journal_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&journal_path)?;

        for entry in entries.iter() {
            let line = serde_json::json!({
                "skill_name": entry.skill_name,
                "tool_name": entry.tool_name,
                "timestamp_secs": entry.timestamp_secs,
                "success": entry.success,
                "user_correction": entry.user_correction,
                "execution_time_ms": entry.execution_time_ms,
                "notes": entry.notes,
            });
            writeln!(file, "{}", line)?;
        }

        Ok(())
    }
}

/// Analyze skill journal and generate improvement suggestions.
/// Returns a map of skill_name → suggested improvements.
pub async fn analyze_skill_improvements(
    journal: &SkillJournal,
    provider: Arc<dyn crate::providers::Provider>,
    model_name: &str,
) -> HashMap<String, String> {
    let mut improvements = HashMap::new();
    
    // Collect all skill names from journal
    let skill_names = {
        let entries = journal.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut names = std::collections::HashSet::new();
        for entry in entries.iter() {
            names.insert(entry.skill_name.clone());
        }
        names.into_iter().collect::<Vec<_>>()
    };

    for skill_name in skill_names {
        if !journal.should_improve(&skill_name, 3) {
            continue;
        }

        let (total, successes, corrections) = journal.get_skill_stats(&skill_name);
        let failure_rate = if total > 0 {
            (total - successes) as f64 / total as f64
        } else {
            0.0
        };

        if failure_rate < 0.2 && corrections.is_empty() {
            continue; // Skill is performing well
        }

        let corrections_str = if corrections.is_empty() {
            "none".to_string()
        } else {
            corrections.join("; ")
        };
        let prompt = format!(
            "You are a skill improvement engine. A skill named '{}' has been used {} times \
             with a {:.0}% failure rate. User corrections: {}\n\n\
             Suggest concrete improvements to the skill's implementation or description \
             to reduce failures and address user feedback. \
             Respond with bullet points only. Max 300 chars.",
            skill_name,
            total,
            failure_rate * 100.0,
            corrections_str
        );

        let request = crate::providers::ChatRequest {
            messages: &[crate::providers::ChatMessage::user(prompt)],
            model: model_name,
            temperature: 0.3,
            max_tokens: Some(300),
            tools: None,
            timeout_secs: 30,
            reasoning_effort: None,
        };

        if let Ok(resp) = provider.chat(&request) {
            if let Some(content) = resp.content {
                improvements.insert(skill_name, content);
            }
        }
    }

    improvements
}

/// Rewrite a skill file with improved content based on analysis.
pub async fn improve_skill_file(
    skill_path: &Path,
    improvements: &str,
    provider: Arc<dyn crate::providers::Provider>,
    model_name: &str,
) -> anyhow::Result<bool> {
    let existing = tokio::fs::read_to_string(skill_path).await?;
    
    let prompt = format!(
        "You are a skill rewriting engine. Given the existing skill definition and improvement suggestions, \
         produce an updated skill definition.\n\n\
         Existing skill:\n{}\n\n\
         Improvements:\n{}\n\n\
         Rewrite the skill incorporating these improvements. \
         Keep the same overall structure. Output ONLY the updated skill definition.",
        existing, improvements
    );

    let request = crate::providers::ChatRequest {
        messages: &[crate::providers::ChatMessage::user(prompt)],
        model: model_name,
        temperature: 0.2,
        max_tokens: Some(2000),
        tools: None,
        timeout_secs: 45,
        reasoning_effort: None,
    };

    match provider.chat(&request) {
        Ok(resp) => {
            if let Some(updated) = resp.content {
                if updated != existing && !updated.is_empty() {
                    tokio::fs::write(skill_path, updated).await?;
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Err(_) => Ok(false),
    }
}

/// Load historical skill journal from disk.
pub fn load_journal(workspace_dir: &str) -> SkillJournal {
    let journal = SkillJournal::new(workspace_dir.to_string());
    let journal_path = Path::new(workspace_dir).join("state").join("skill_journal.jsonl");
    
    if let Ok(content) = std::fs::read_to_string(&journal_path) {
        for line in content.lines() {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(exec) = parse_execution_from_json(&value) {
                    journal.record(exec);
                }
            }
        }
    }

    journal
}

fn parse_execution_from_json(value: &serde_json::Value) -> Option<SkillExecution> {
    Some(SkillExecution {
        skill_name: value.get("skill_name")?.as_str()?.to_string(),
        tool_name: value.get("tool_name")?.as_str()?.to_string(),
        timestamp_secs: value.get("timestamp_secs")?.as_u64()?,
        success: value.get("success")?.as_bool()?,
        user_correction: value.get("user_correction").and_then(|v| v.as_str().map(|s| s.to_string())),
        execution_time_ms: value.get("execution_time_ms")?.as_u64()?,
        notes: value.get("notes")?.as_str()?.to_string(),
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

use std::io::Write;
