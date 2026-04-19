use super::Provider;
use crate::providers::ChatMessage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

// NOTE: Cooldown is now tracked per-agent via Agent::last_compaction (u64, unix secs).
// The old process-global static is removed — it blocked all agents when one compacted.
const COMPACTION_COOLDOWN_SECS: u64 = 300; // 5 minutes

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

pub fn token_estimate(history: &[ChatMessage], model_name: &str) -> u64 {
    let chars_per_token = match model_name.to_lowercase().as_str() {
        s if s.contains("claude") => 3.5,
        s if s.contains("gemini") => 4.5,
        s if s.contains("gpt-4") => 4.0,
        s if s.contains("gpt-3") => 4.0,
        _ => 4.0,
    };

    let mut total_chars = 0u64;
    for msg in history {
        total_chars += msg.content.len() as u64;
        total_chars += 10; // role/meta overhead
    }

    (total_chars as f64 / chars_per_token).ceil() as u64
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

/// `last_compaction` is a per-agent timestamp (unix secs) tracking when this agent
/// last compacted. Callers must pass `&mut agent.last_compaction` so the cooldown
/// is isolated per agent rather than process-global.
pub fn auto_compact_history(
    history: &mut Vec<ChatMessage>,
    provider: &Arc<dyn Provider>,
    model_name: &str,
    config: &CompactionConfig,
    last_compaction: &mut u64,
) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if now - *last_compaction < COMPACTION_COOLDOWN_SECS {
        return false; // Per-agent cooldown active
    }

    let has_system = !history.is_empty() && history[0].role == "system";
    let start = if has_system { 1 } else { 0 };
    let non_system_count = history.len() - start;

    let is_small = crate::agent::context_tokens::is_small_model_context(config.token_limit);
    let count_trigger = non_system_count > config.max_history_messages as usize;

    // Add a 10% buffer to the token threshold to avoid edge-case "jitter" summarization
    // For small models, trigger even earlier (at 50% instead of 75%)
    let threshold_num = if is_small { 1 } else { 3 };
    let threshold_den = if is_small { 2 } else { 4 };
    let token_threshold = (config.token_limit * threshold_num) / threshold_den;

    let token_trigger =
        config.token_limit > 0 && token_estimate(history, model_name) > token_threshold;

    // Only compact if we are significantly over the threshold or have many messages
    if !count_trigger && !token_trigger {
        return false;
    }

    let keep_recent = std::cmp::min(config.keep_recent as usize, non_system_count);
    let compact_count = non_system_count - keep_recent;
    if compact_count == 0 {
        return false;
    }

    let mut compact_end = start + compact_count;

    let summary = summarize_chunked(provider, model_name, history, start, compact_end, config)
        .unwrap_or_else(|| "Context summarized due to length.".to_string());

    *last_compaction = now; // Update per-agent cooldown timestamp

    // Construct the final summary message content
    // Potentially read workspace context here (AGENTS.md)
    let mut summary_content = format!("[Compaction summary]\n{}", summary);
    if let Some(workspace_context) =
        read_workspace_context_for_summary(config.workspace_dir.as_deref())
        && !workspace_context.is_empty() {
            summary_content.push_str("\n\n");
            summary_content.push_str(&workspace_context);
        }

    // Advance compact_end to the next "user" message to ensure the kept history starts with a user prompt.
    // Strict APIs like Gemini and Anthropic will reject histories that start with "assistant" or don't alternate.
    while compact_end < history.len() && history[compact_end].role != "user" {
        compact_end += 1;
    }

    if compact_end >= history.len() {
        return false; // Cannot compact without breaking the conversation completely
    }

    let mut new_history = Vec::new();

    if has_system {
        let mut sys_msg = history[0].clone();
        sys_msg
            .content
            .push_str("\n\n--- COMPACTED PAST CONTEXT ---\n");
        sys_msg.content.push_str(&summary_content);
        new_history.push(sys_msg);
    } else {
        new_history.push(ChatMessage {
            role: "system".to_string(),
            content: summary_content,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
                    thought_signature: None,
        });
    }

    new_history.extend_from_slice(&history[compact_end..]);

    *history = new_history;
    true
}

/// Multi-part summarization for very large histories.
///
/// If the transcript fits in one chunk, delegates to `summarize_slice`.
/// Otherwise splits into overlapping chunks, summarises each independently,
/// then collapses the partial summaries into a single final summary.
/// This avoids the quality degradation of feeding a 50k-char transcript to
/// a 512-token summarizer in a single shot.
fn summarize_chunked(
    provider: &Arc<dyn Provider>,
    model_name: &str,
    history: &[ChatMessage],
    start: usize,
    end: usize,
    config: &CompactionConfig,
) -> Option<String> {
    let transcript = build_compaction_transcript(history, start, end, u32::MAX);
    let max_chunk = config.max_source_chars as usize;

    // Single-pass fast path
    if transcript.len() <= max_chunk {
        return summarize_slice(provider, model_name, history, start, end, config);
    }

    // Split transcript into overlapping char-based chunks
    let overlap = max_chunk / 5; // 20% overlap
    let chars: Vec<char> = transcript.chars().collect();
    let total = chars.len();
    let mut chunk_summaries: Vec<String> = Vec::new();
    let mut pos = 0;

    while pos < total {
        let chunk_end = (pos + max_chunk).min(total);
        let chunk_text: String = chars[pos..chunk_end].iter().collect();

        let summarizer_system = "You are a conversation compaction engine. Summarize this excerpt of older chat history into concise bullet points. Preserve: user preferences, commitments, decisions, unresolved tasks, key facts. Omit: filler and verbose tool logs. Output plain text bullet points only.";
        let summarizer_user = format!(
            "Summarize this conversation excerpt (part {}/{}):\n\n{}",
            chunk_summaries.len() + 1,
            ((total + max_chunk - 1) / (max_chunk - overlap)).max(1),
            chunk_text
        );

        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: summarizer_system.to_string(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
                thought_signature: None,
            },
            ChatMessage {
                role: "user".to_string(),
                content: summarizer_user,
                name: None,
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
                thought_signature: None,
            },
        ];

        let provider_clone = provider.clone();
        let model_owned = model_name.to_string();
        let partial: Option<String> = tokio::task::block_in_place(|| {
            let req = crate::providers::ChatRequest {
                messages: &messages,
                model: &model_owned,
                temperature: 0.2,
                max_tokens: Some(256),
                tools: None,
                timeout_secs: 45,
                reasoning_effort: None,
            };
            match provider_clone.chat(&req) {
                Ok(resp) => resp.content,
                Err(_) => {
                    // Fallback: truncate chunk
                    let safe = chunk_text
                        .char_indices()
                        .nth(config.max_summary_chars as usize / 2)
                        .map(|(i, _)| i)
                        .unwrap_or(chunk_text.len());
                    Some(chunk_text[..safe].to_string())
                }
            }
        });

        if let Some(s) = partial {
            chunk_summaries.push(s);
        }

        if chunk_end == total {
            break;
        }
        pos += max_chunk - overlap;
    }

    if chunk_summaries.is_empty() {
        return None;
    }

    // Single-chunk result: return directly
    if chunk_summaries.len() == 1 {
        return Some(chunk_summaries.remove(0));
    }

    // Merge partial summaries into one final summary
    let combined = chunk_summaries.join("\n\n---\n\n");
    let merge_system = "You are a conversation compaction engine. You have been given several partial summaries of a conversation. Merge them into a single concise summary of at most 12 bullet points. Remove duplicates, preserve key facts, decisions, and unresolved tasks.";
    let merge_user = format!("Merge these partial conversation summaries:\n\n{}", combined);

    let merge_messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: merge_system.to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
            thought_signature: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: merge_user,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
            thought_signature: None,
        },
    ];

    let provider_clone = provider.clone();
    let model_owned = model_name.to_string();
    let max_summary_chars = config.max_summary_chars;

    tokio::task::block_in_place(|| {
        let req = crate::providers::ChatRequest {
            messages: &merge_messages,
            model: &model_owned,
            temperature: 0.2,
            max_tokens: Some(512),
            tools: None,
            timeout_secs: 60,
            reasoning_effort: None,
        };
        match provider_clone.chat(&req) {
            Ok(resp) => resp.content,
            Err(_) => {
                // Fallback: join partial summaries and truncate
                let joined = chunk_summaries.join("\n");
                let safe = joined
                    .char_indices()
                    .nth(max_summary_chars as usize)
                    .map(|(i, _)| i)
                    .unwrap_or(joined.len());
                Some(joined[..safe].to_string())
            }
        }
    })
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
    let summarizer_user = format!(
        "Summarize the following conversation history for context preservation. Keep it short (max 12 bullet points).\n\n{}",
        transcript
    );

    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: summarizer_system.to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
                    thought_signature: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: summarizer_user,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
                    thought_signature: None,
        },
    ];

    // DESIGN-3: Use block_in_place so the blocking provider.chat() call does not
    // starve the tokio thread pool. Compaction is called from async context.
    let provider_clone = provider.clone();
    let model_name_owned = model_name.to_string();
    let max_summary_chars = config.max_summary_chars;

    // Build an owned request for use inside block_in_place
    let messages_owned = messages;
    let result: Option<String> = tokio::task::block_in_place(|| {
        let request = crate::providers::ChatRequest {
            messages: &messages_owned,
            model: &model_name_owned,
            temperature: 0.2,
            max_tokens: Some(512),
            tools: None,
            timeout_secs: 60,
            reasoning_effort: None,
        };
        match provider_clone.chat(&request) {
            Ok(resp) => resp.content,
            Err(_) => {
                // Fallback: truncate transcript
                let safe_len = transcript
                    .char_indices()
                    .nth(max_summary_chars as usize)
                    .map(|(i, _)| i)
                    .unwrap_or(transcript.len());
                Some(transcript[..safe_len].to_string())
            }
        }
    });
    result
}

fn build_compaction_transcript(
    history: &[ChatMessage],
    start: usize,
    end: usize,
    max_chars: u32,
) -> String {
    let mut buf = String::new();
    for msg in &history[start..end] {
        let role = msg.role.to_uppercase();
        buf.push_str(&role);
        buf.push_str(": ");
        let safe_500 = msg
            .content
            .char_indices()
            .nth(500)
            .map(|(i, _)| i)
            .unwrap_or(msg.content.len());
        let content = &msg.content[..safe_500];
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
        let safe_2000 = content
            .char_indices()
            .nth(2000)
            .map(|(i, _)| i)
            .unwrap_or(content.len());
        if content.len() > safe_2000 {
            Some(format!("{}\n...[truncated]...", &content[..safe_2000]))
        } else {
            Some(content)
        }
    } else {
        None
    }
}
