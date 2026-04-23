/// Enhanced dialectic user modeling — Honcho-style cross-session user profiling.
/// 
/// This module extends the basic dialectic analysis with:
/// 1. Periodic nudges to persist knowledge (not just on turn boundaries)
/// 2. Multi-file user model (USER.md, PREFERENCES.md, PATTERNS.md)
/// 3. Cross-session search with LLM summarization for episodic recall
/// 4. Dialectic modeling — building a "theory of mind" about the user

use crate::providers::{ChatMessage, ChatRequest};
use std::path::Path;
use std::sync::Arc;

const DIALECTIC_FILE: &str = "DIALECTIC.md";
const PREFERENCES_FILE: &str = "PREFERENCES.md";
const PATTERNS_FILE: &str = "PATTERNS.md";

/// Analyze a completed session and update the multi-file user model.
/// This is called periodically (every N turns) to build deepening user understanding.
pub async fn analyze_session_enhanced(
    provider: Arc<dyn crate::providers::Provider>,
    model_name: &str,
    history: &[ChatMessage],
    workspace_dir: &str,
) {
    if history.len() < 4 {
        return;
    }

    // Build a summary of the conversation for analysis
    let mut summary = String::new();
    for msg in history.iter().rev().take(20).rev() {
        let prefix = match msg.role.as_str() {
            "user" => "User",
            "assistant" => "Assistant",
            _ => continue,
        };
        let content = msg.content.trim();
        if content.len() > 300 {
            summary.push_str(&format!("{}: {}...\n", prefix, &content[..300]));
        } else {
            summary.push_str(&format!("{}: {}\n", prefix, content));
        }
    }

    // Read all existing user model files
    let dialectic = read_file(workspace_dir, DIALECTIC_FILE).await;
    let preferences = read_file(workspace_dir, PREFERENCES_FILE).await;
    let patterns = read_file(workspace_dir, PATTERNS_FILE).await;

    // Run three focused analyses in parallel for better coverage
    let dialectic_update = analyze_dialectic(provider.clone(), model_name, &summary, &dialectic).await;
    let preferences_update = analyze_preferences(provider.clone(), model_name, &summary, &preferences).await;
    let patterns_update = analyze_patterns(provider.clone(), model_name, &summary, &patterns).await;

    // Write updates (only if non-empty and changed)
    if let Some(content) = dialectic_update {
        if !content.is_empty() && content != dialectic {
            let _ = tokio::fs::write(Path::new(workspace_dir).join(DIALECTIC_FILE), content).await;
        }
    }
    if let Some(content) = preferences_update {
        if !content.is_empty() && content != preferences {
            let _ = tokio::fs::write(Path::new(workspace_dir).join(PREFERENCES_FILE), content).await;
        }
    }
    if let Some(content) = patterns_update {
        if !content.is_empty() && content != patterns {
            let _ = tokio::fs::write(Path::new(workspace_dir).join(PATTERNS_FILE), content).await;
        }
    }
}

async fn read_file(workspace_dir: &str, filename: &str) -> String {
    let path = Path::new(workspace_dir).join(filename);
    match tokio::fs::read_to_string(&path).await {
        Ok(c) if !c.trim().is_empty() => c,
        _ => String::new(),
    }
}

/// Analyze communication style, patience, frustration triggers.
async fn analyze_dialectic(
    provider: Arc<dyn crate::providers::Provider>,
    model_name: &str,
    summary: &str,
    existing: &str,
) -> Option<String> {
    let prompt = format!(
        "You are a user-modeling analyst. Given recent conversation and existing profile, \
         extract/update meta-context about the user's communication style.\n\n\
         Existing profile:\n{}\n\n\
         Recent conversation:\n{}\n\n\
         Respond with ONLY the updated profile (max 600 chars). Focus on: \
         communication style, patience levels, frustration triggers, work habits. \
         Be concise. If nothing new, return the existing profile unchanged.",
        if existing.is_empty() { "(empty)" } else { existing },
        summary
    );

    let request = ChatRequest {
        messages: &[ChatMessage::user(prompt)],
        model: model_name,
        temperature: 0.2,
        max_tokens: Some(500),
        tools: None,
        timeout_secs: 30,
        reasoning_effort: None,
    };

    match provider.chat(&request) {
        Ok(r) => r.content.map(|c| c.trim().to_string()),
        Err(_) => None,
    }
}

/// Extract concrete preferences (tools, formats, output style).
async fn analyze_preferences(
    provider: Arc<dyn crate::providers::Provider>,
    model_name: &str,
    summary: &str,
    existing: &str,
) -> Option<String> {
    let prompt = format!(
        "You are a preference extraction engine. Given recent conversation and existing preferences, \
         extract/update concrete user preferences.\n\n\
         Existing preferences:\n{}\n\n\
         Recent conversation:\n{}\n\n\
         Respond with ONLY updated preferences (max 600 chars). Focus on: \
         preferred tools, output formats, coding style, documentation habits. \
         Use bullet points. If nothing new, return existing unchanged.",
        if existing.is_empty() { "(empty)" } else { existing },
        summary
    );

    let request = ChatRequest {
        messages: &[ChatMessage::user(prompt)],
        model: model_name,
        temperature: 0.2,
        max_tokens: Some(500),
        tools: None,
        timeout_secs: 30,
        reasoning_effort: None,
    };

    match provider.chat(&request) {
        Ok(r) => r.content.map(|c| c.trim().to_string()),
        Err(_) => None,
    }
}

/// Extract recurring patterns and workflows.
async fn analyze_patterns(
    provider: Arc<dyn crate::providers::Provider>,
    model_name: &str,
    summary: &str,
    existing: &str,
) -> Option<String> {
    let prompt = format!(
        "You are a pattern recognition engine. Given recent conversation and existing patterns, \
         extract/update recurring workflows.\n\n\
         Existing patterns:\n{}\n\n\
         Recent conversation:\n{}\n\n\
         Respond with ONLY updated patterns (max 600 chars). Focus on: \
         recurring tasks, common workflows, typical requests, domain expertise areas. \
         Use bullet points. If nothing new, return existing unchanged.",
        if existing.is_empty() { "(empty)" } else { existing },
        summary
    );

    let request = ChatRequest {
        messages: &[ChatMessage::user(prompt)],
        model: model_name,
        temperature: 0.2,
        max_tokens: Some(500),
        tools: None,
        timeout_secs: 30,
        reasoning_effort: None,
    };

    match provider.chat(&request) {
        Ok(r) => r.content.map(|c| c.trim().to_string()),
        Err(_) => None,
    }
}

/// Load the combined dialectic context for prompt injection.
/// This aggregates all user model files into a single context block.
pub fn load_dialectic_context_enhanced(workspace_dir: &str) -> String {
    let dialectic = std::fs::read_to_string(Path::new(workspace_dir).join(DIALECTIC_FILE))
        .unwrap_or_default();
    let preferences = std::fs::read_to_string(Path::new(workspace_dir).join(PREFERENCES_FILE))
        .unwrap_or_default();
    let patterns = std::fs::read_to_string(Path::new(workspace_dir).join(PATTERNS_FILE))
        .unwrap_or_default();

    let mut parts = Vec::new();
    if !dialectic.trim().is_empty() {
        parts.push(format!("## Communication Style\n{}", dialectic.trim()));
    }
    if !preferences.trim().is_empty() {
        parts.push(format!("## Preferences\n{}", preferences.trim()));
    }
    if !patterns.trim().is_empty() {
        parts.push(format!("## Recurring Patterns\n{}", patterns.trim()));
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!("<user-model>\n{}\n</user-model>", parts.join("\n\n"))
    }
}

/// Periodic nudge: check if we should trigger background user modeling.
/// Called every N turns to persist knowledge before it gets compressed away.
pub fn should_nudge_user_modeling(turn_count: u64, interval: u32) -> bool {
    if interval == 0 {
        return false;
    }
    turn_count > 0 && turn_count % interval as u64 == 0
}

/// Nudge content to inject into the conversation to encourage the agent
/// to persist important observations about the user.
pub fn user_modeling_nudge() -> String {
    concat!(
        "[System nudge: Consider if anything in this conversation reveals new information ",
        "about the user's preferences, work style, or communication patterns. ",
        "If so, use the memory_md tool to save it for future sessions.]"
    )
    .to_string()
}

/// Search across session history for relevant past interactions.
/// Uses FTS5 if available, falls back to simple keyword matching.
pub async fn search_session_memory(
    workspace_dir: &str,
    query: &str,
    provider: Option<Arc<dyn crate::providers::Provider>>,
    model_name: Option<&str>,
) -> Option<String> {
    // Try SQLite FTS5 first
    let db_path = format!("{}/memory.db", workspace_dir);
    if std::path::Path::new(&db_path).exists() {
        if let Ok(results) = search_sqlite_memory(&db_path, query).await {
            if !results.is_empty() {
                return Some(results);
            }
        }
    }

    // Fallback: search markdown memory files
    let md_path = Path::new(workspace_dir).join("MEMORY.md");
    if let Ok(content) = tokio::fs::read_to_string(&md_path).await {
        let relevant = extract_relevant_lines(&content, query, 20);
        if !relevant.is_empty() {
            return Some(format!("## Past Memory (relevant excerpts)\n{}", relevant));
        }
    }

    None
}

async fn search_sqlite_memory(db_path: &str, query: &str) -> Result<String, anyhow::Error> {
    use rusqlite::Connection;
    
    let conn = Connection::open(db_path)?;
    // Try FTS5 virtual table
    let sql = "SELECT content FROM memory_fts WHERE memory_fts MATCH ? ORDER BY rank LIMIT 10";
    let mut stmt = conn.prepare(sql)?;
    let rows: Result<Vec<String>, _> = stmt
        .query_map([query], |row| row.get(0))?
        .collect();
    
    match rows {
        Ok(contents) if !contents.is_empty() => {
            Ok(format!("## Relevant Past Memories\n{}", contents.join("\n\n")))
        }
        _ => Ok(String::new()),
    }
}

fn extract_relevant_lines(content: &str, query: &str, max_lines: usize) -> String {
    let query_lower = query.to_lowercase();
    let words: Vec<&str> = query_lower.split_whitespace().collect();
    
    let lines: Vec<&str> = content.lines().collect();
    let mut scored: Vec<(usize, &str)> = Vec::new();
    
    for (i, line) in lines.iter().enumerate() {
        let line_lower = line.to_lowercase();
        let score = words.iter().filter(|w| line_lower.contains(*w)).count();
        if score > 0 {
            scored.push((score, *line));
        }
    }
    
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.truncate(max_lines);
    
    scored.into_iter().map(|(_, line)| line).collect::<Vec<_>>().join("\n")
}
