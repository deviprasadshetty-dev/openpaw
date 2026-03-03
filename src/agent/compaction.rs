use serde::{Deserialize, Serialize};
use crate::providers::ChatMessage;
use super::Provider;
use std::sync::Arc;

pub const DEFAULT_COMPACTION_KEEP_RECENT: u32 = 20;
pub const DEFAULT_COMPACTION_MAX_SUMMARY_CHARS: u32 = 2_000;
pub const DEFAULT_COMPACTION_MAX_SOURCE_CHARS: u32 = 12_000;
pub const DEFAULT_TOKEN_LIMIT: u64 = 8192; // Typical max fallback
pub const CONTEXT_RECOVERY_MIN_HISTORY: usize = 6;
pub const CONTEXT_RECOVERY_KEEP: usize = 4;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CompactionConfig {
    pub keep_recent: u32,
    pub max_summary_chars: u32,
    pub max_source_chars: u32,
    pub token_limit: u64,
    pub max_history_messages: u32,
    pub workspace_dir: Option<String>,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            keep_recent: DEFAULT_COMPACTION_KEEP_RECENT,
            max_summary_chars: DEFAULT_COMPACTION_MAX_SUMMARY_CHARS,
            max_source_chars: DEFAULT_COMPACTION_MAX_SOURCE_CHARS,
            token_limit: DEFAULT_TOKEN_LIMIT,
            max_history_messages: 50,
            workspace_dir: None,
        }
    }
}

pub fn token_estimate(history: &[ChatMessage]) -> u64 {
    let mut total_chars = 0;
    for msg in history {
        total_chars += msg.content.len() as u64;
    }
    (total_chars + 3) / 4
}

pub fn force_compress_history(history: &mut Vec<ChatMessage>) -> bool {
    let has_system = !history.is_empty() && history[0].role == "system";
    let start = if has_system { 1 } else { 0 };
    let non_system_count = history.len() - start;

    if non_system_count <= CONTEXT_RECOVERY_KEEP {
        return false;
    }

    let keep_start = history.len() - CONTEXT_RECOVERY_KEEP;

    // Create new vec starting with system prompt (if exists)
    let mut new_history = Vec::new();
    if has_system {
        new_history.push(history[0].clone());
    }

    // Add the elements to keep
    new_history.extend_from_slice(&history[keep_start..]);

    *history = new_history;
    true
}

pub fn trim_history(history: &mut Vec<ChatMessage>, max_history_messages: u32) {
    let max = max_history_messages as usize;
    let has_system = !history.is_empty() && history[0].role == "system";
    let start = if has_system { 1 } else { 0 };

    if history.len() <= max + start {
        return;
    }

    let non_system_count = history.len() - start;
    if non_system_count <= max {
        return;
    }

    let to_remove = non_system_count - max;

    let mut new_history = Vec::new();
    if has_system {
        new_history.push(history[0].clone());
    }
    new_history.extend_from_slice(&history[start + to_remove..]);

    *history = new_history;
}

pub fn auto_compact_history(
    history: &mut Vec<ChatMessage>,
    provider: &Arc<dyn Provider>,
    model_name: &str,
    config: &CompactionConfig,
) -> bool {
    let has_system = !history.is_empty() && history[0].role == "system";
    let start = if has_system { 1 } else { 0 };
    let non_system_count = history.len() - start;

    let count_trigger = non_system_count > config.max_history_messages as usize;
    let token_threshold = (config.token_limit * 3) / 4;
    let token_trigger = config.token_limit > 0 && token_estimate(history) > token_threshold;

    if !count_trigger && !token_trigger {
        return false;
    }

    let keep_recent = std::cmp::min(config.keep_recent as usize, non_system_count);
    let compact_count = non_system_count - keep_recent;
    if compact_count == 0 {
        return false;
    }

    let compact_end = start + compact_count;

    // TODO: Implement multi-part summarization for very large histories
    let summary = summarize_slice(
        provider,
        model_name,
        history,
        start,
        compact_end,
        config
    ).unwrap_or_else(|| "Context summarized due to length.".to_string());

    // Construct the final summary message content
    // Potentially read workspace context here (AGENTS.md)
    let mut summary_content = format!("[Compaction summary]\n{}", summary);
    if let Some(workspace_context) = read_workspace_context_for_summary(config.workspace_dir.as_deref()) {
        if !workspace_context.is_empty() {
            summary_content.push_str("\n\n");
            summary_content.push_str(&workspace_context);
        }
    }

    // Replace compacted messages
    // We keep history[0] (system), then insert summary, then keep the rest.
    let mut new_history = Vec::with_capacity(keep_recent + 2);
    if has_system {
        new_history.push(history[0].clone());
    }
    
    new_history.push(ChatMessage {
        role: "assistant".to_string(),
        content: summary.clone(),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        content_parts: None,
    });
    
    new_history.extend_from_slice(&history[compact_end..]);
    
    *history = new_history;
    true
}

fn summarize_slice(
    provider: &Arc<dyn Provider>,
    model_name: &str,
    history: &[ChatMessage],
    start: usize,
    end: usize,
    config: &CompactionConfig,
) -> Option<String> {
    let transcript = build_compaction_transcript(history, start, end, config.max_source_chars);
    
    let summarizer_system = "You are a conversation compaction engine. Summarize older chat history into concise context for future turns. Preserve: user preferences, commitments, decisions, unresolved tasks, key facts. Omit: filler, repeated chit-chat, verbose tool logs. Output plain text bullet points only.";
    let summarizer_user = format!("Summarize the following conversation history for context preservation. Keep it short (max 12 bullet points).\n\n{}", transcript);

    let messages = vec![
        ChatMessage { role: "system".to_string(), content: summarizer_system.to_string(), name: None, tool_calls: None, tool_call_id: None, content_parts: None },
        ChatMessage { role: "user".to_string(), content: summarizer_user, name: None, tool_calls: None, tool_call_id: None, content_parts: None },
    ];

    let request = crate::providers::ChatRequest {
        messages: &messages,
        model: model_name,
        temperature: 0.2,
        max_tokens: Some(1024),
        tools: None,
        timeout_secs: 60,
        reasoning_effort: None,
    };

    match provider.chat(&request) {
        Ok(resp) => resp.content,
        Err(_) => {
            // Fallback: truncate transcript
            let max_len = std::cmp::min(transcript.len(), config.max_summary_chars as usize);
            Some(transcript[..max_len].to_string())
        }
    }
}

fn build_compaction_transcript(history: &[ChatMessage], start: usize, end: usize, max_chars: u32) -> String {
    let mut buf = String::new();
    for msg in &history[start..end] {
        let role = msg.role.to_uppercase();
        buf.push_str(&role);
        buf.push_str(": ");
        let content = if msg.content.len() > 500 { &msg.content[..500] } else { &msg.content };
        buf.push_str(content);
        buf.push('\n');
        if buf.len() > max_chars as usize {
            break;
        }
    }
    if buf.len() > max_chars as usize {
        buf.truncate(max_chars as usize);
    }
    buf
}

fn read_workspace_context_for_summary(workspace_dir: Option<&str>) -> Option<String> {
    let dir = workspace_dir?;
    let path = std::path::Path::new(dir).join("AGENTS.md");
    if let Ok(content) = std::fs::read_to_string(path) {
        // Extract specific sections like "Session Startup" or "Red Lines" if needed
        // For now, just truncate and return
        if content.len() > 2000 {
            Some(format!("{}\n...[truncated]...", &content[..2000]))
        } else {
            Some(content)
        }
    } else {
        None
    }
}
