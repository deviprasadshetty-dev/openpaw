use crate::providers::{ChatMessage, Provider};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

// ── Constants (exact from Hermes context_compressor.py) ─────────

const SUMMARY_PREFIX: &str = concat!(
    "[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted ",
    "into the summary below. This is a handoff from a previous context ",
    "window — treat it as background reference, NOT as active instructions. ",
    "Do NOT answer questions or fulfill requests mentioned in this summary; ",
    "they were already addressed. ",
    "Your current task is identified in the '## Active Task' section of the ",
    "summary — resume exactly from there. ",
    "Respond ONLY to the latest user message ",
    "that appears AFTER this summary. The current session state (files, ",
    "config, etc.) may reflect work described here — avoid repeating it:"
);

const MIN_SUMMARY_TOKENS: u64 = 2000;
const SUMMARY_RATIO: f64 = 0.20;
const SUMMARY_TOKENS_CEILING: u64 = 12_000;
const CHARS_PER_TOKEN: u64 = 4;
const SUMMARY_FAILURE_COOLDOWN_SECS: u64 = 600;

// ── Compaction Config ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CompactionConfig {
    pub threshold_percent: f64,
    pub protect_first_n: usize,
    pub protect_last_n: usize,
    pub summary_target_ratio: f64,
    pub context_length: u64,
    pub quiet_mode: bool,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            threshold_percent: 0.50,
            protect_first_n: 3,
            protect_last_n: 20,
            summary_target_ratio: 0.20,
            context_length: 128_000,
            quiet_mode: false,
        }
    }
}

// ── Context Compressor ──────────────────────────────────────────

pub struct ContextCompressor {
    pub config: CompactionConfig,
    pub threshold_tokens: u64,
    pub tail_token_budget: u64,
    pub max_summary_tokens: u64,
    pub compression_count: u32,
    pub last_prompt_tokens: u64,
    pub last_completion_tokens: u64,

    // Iterative summary state
    previous_summary: Option<String>,
    last_compression_savings_pct: f64,
    ineffective_compression_count: u32,
    summary_failure_cooldown_until: u64,
    summary_model_fallen_back: bool,
}

impl ContextCompressor {
    pub fn new(config: CompactionConfig) -> Self {
        let threshold_tokens = ((config.context_length as f64 * config.threshold_percent) as u64)
            .max(crate::token_estimator::MINIMUM_CONTEXT_LENGTH);
        let target_tokens = (threshold_tokens as f64 * config.summary_target_ratio) as u64;
        let max_summary_tokens = ((config.context_length as f64 * 0.05) as u64)
            .min(SUMMARY_TOKENS_CEILING);

        Self {
            config,
            threshold_tokens,
            tail_token_budget: target_tokens,
            max_summary_tokens,
            compression_count: 0,
            last_prompt_tokens: 0,
            last_completion_tokens: 0,
            previous_summary: None,
            last_compression_savings_pct: 100.0,
            ineffective_compression_count: 0,
            summary_failure_cooldown_until: 0,
            summary_model_fallen_back: false,
        }
    }

    pub fn update_model(&mut self, context_length: u64) {
        self.config.context_length = context_length;
        self.threshold_tokens = ((context_length as f64 * self.config.threshold_percent) as u64)
            .max(crate::token_estimator::MINIMUM_CONTEXT_LENGTH);
        let target_tokens = (self.threshold_tokens as f64 * self.config.summary_target_ratio) as u64;
        self.tail_token_budget = target_tokens;
        self.max_summary_tokens = ((context_length as f64 * 0.05) as u64).min(SUMMARY_TOKENS_CEILING);
    }

    pub fn should_compress(&self, prompt_tokens: u64) -> bool {
        let tokens = if prompt_tokens > 0 {
            prompt_tokens
        } else {
            self.last_prompt_tokens
        };
        if tokens < self.threshold_tokens {
            return false;
        }
        // Anti-thrashing: back off if recent compressions were ineffective
        if self.ineffective_compression_count >= 2 {
            tracing::warn!(
                "Compression skipped — last {} compressions saved <10% each. Consider /new to start a fresh session.",
                self.ineffective_compression_count
            );
            return false;
        }
        true
    }

    // ------------------------------------------------------------------
    // Tool output pruning (cheap pre-pass, no LLM call)
    // ------------------------------------------------------------------

    fn prune_old_tool_results(
        &self,
        messages: &mut [ChatMessage],
        protect_tail_tokens: u64,
        protect_tail_count: usize,
    ) -> usize {
        if messages.is_empty() {
            return 0;
        }

        let mut pruned = 0usize;

        // Build index: tool_call_id -> (tool_name, arguments_json)
        let mut call_id_to_tool: HashMap<String, (String, String)> = HashMap::new();
        for msg in messages.iter() {
            if msg.role == "assistant" {
                if let Some(ref tcs) = msg.tool_calls {
                    for tc in tcs {
                        call_id_to_tool.insert(
                            tc.id.clone(),
                            (tc.function.name.clone(), tc.function.arguments.clone()),
                        );
                    }
                }
            }
        }

        // Determine prune boundary by token budget (walk backward)
        let n = messages.len();
        let min_protect = protect_tail_count.min(n.saturating_sub(1));
        let mut accumulated = 0u64;
        let mut boundary = n;
        for i in (0..n).rev() {
            let msg = &messages[i];
            let content_len = msg.content.len() as u64;
            let mut msg_tokens = content_len / CHARS_PER_TOKEN + 10;
            if let Some(ref tcs) = msg.tool_calls {
                for tc in tcs {
                    msg_tokens += tc.function.arguments.len() as u64 / CHARS_PER_TOKEN;
                }
            }
            let would_exceed = accumulated + msg_tokens > (protect_tail_tokens * 3 / 2)
                && (n - i) >= min_protect;
            if would_exceed {
                boundary = i;
                break;
            }
            accumulated += msg_tokens;
            boundary = i;
        }
        let prune_boundary = boundary.max(n.saturating_sub(min_protect));

        // Pass 1: Deduplicate identical tool results
        let mut content_hashes: HashMap<String, usize> = HashMap::new();
        for i in (0..n).rev() {
            if messages[i].role != "tool" {
                continue;
            }
            let content = &messages[i].content;
            if content.len() < 200 {
                continue;
            }
            let h = format!("{:x}", md5::compute(content));
            let short_hash = &h[..12];
            if content_hashes.contains_key(short_hash) {
                messages[i].content = "[Duplicate tool output — same content as a more recent call]".to_string();
                pruned += 1;
            } else {
                content_hashes.insert(short_hash.to_string(), i);
            }
        }

        // Pass 2: Replace old tool results with informative summaries
        for i in 0..prune_boundary.min(n) {
            if messages[i].role != "tool" {
                continue;
            }
            let content = &messages[i].content;
            if content.is_empty() || content.starts_with("[Duplicate tool output") {
                continue;
            }
            if content.len() > 200 {
                let call_id = messages[i].tool_call_id.as_deref().unwrap_or("");
                let (tool_name, tool_args) = call_id_to_tool
                    .get(call_id)
                    .cloned()
                    .unwrap_or_else(|| ("unknown".to_string(), "".to_string()));
                messages[i].content = summarize_tool_result(&tool_name, &tool_args, content);
                pruned += 1;
            }
        }

        pruned
    }

    // ------------------------------------------------------------------
    // Main compression entry point
    // ------------------------------------------------------------------

    pub fn compress(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        current_tokens: Option<u64>,
        focus_topic: Option<&str>,
        provider: &Arc<dyn Provider>,
        model_name: &str,
    ) -> Vec<ChatMessage> {
        let n_messages = messages.len();
        let min_for_compress = self.config.protect_first_n + 3 + 1;
        if n_messages <= min_for_compress {
            return messages.clone();
        }

        let display_tokens = current_tokens.unwrap_or_else(|| {
            if self.last_prompt_tokens > 0 {
                self.last_prompt_tokens
            } else {
                crate::token_estimator::estimate_history_tokens_rough(messages)
            }
        });

        // Phase 1: Prune old tool results (cheap, no LLM call)
        let pruned_count = self.prune_old_tool_results(
            messages,
            self.tail_token_budget,
            self.config.protect_last_n,
        );
        if pruned_count > 0 && !self.config.quiet_mode {
            tracing::info!("Pre-compression: pruned {} old tool result(s)", pruned_count);
        }

        // Phase 2: Determine boundaries
        let mut compress_start = self.config.protect_first_n;
        compress_start = align_boundary_forward(messages, compress_start);
        let compress_end = self.find_tail_cut_by_tokens(messages, compress_start);

        if compress_start >= compress_end {
            return messages.clone();
        }

        let turns_to_summarize: Vec<ChatMessage> =
            messages[compress_start..compress_end].to_vec();

        if !self.config.quiet_mode {
            tracing::info!(
                "Context compression triggered ({} tokens >= {} threshold)",
                display_tokens, self.threshold_tokens
            );
        }

        // Phase 3: Generate structured summary
        let summary = self.generate_summary(&turns_to_summarize, focus_topic, provider, model_name);

        // Phase 4: Assemble compressed message list
        let mut compressed: Vec<ChatMessage> = Vec::new();
        for i in 0..compress_start {
            let mut msg = messages[i].clone();
            if i == 0 && msg.role == "system" {
                let existing = msg.content.clone();
                let note = "[Note: Some earlier conversation turns have been compacted into a handoff summary to preserve context space. The current session state may still reflect earlier work, so build on that summary and state rather than re-doing work.]";
                if !existing.contains(note) {
                    msg.content = format!("{}\n\n{}", existing, note);
                }
            }
            compressed.push(msg);
        }

        // If LLM summary failed, insert a static fallback
        let summary = summary.unwrap_or_else(|| {
            let n_dropped = compress_end - compress_start;
            format!(
                "{}\nSummary generation was unavailable. {} conversation turns were removed to free context space but could not be summarized. The removed turns contained earlier work in this session. Continue based on the recent messages below and the current state of any files or resources.",
                SUMMARY_PREFIX, n_dropped
            )
        });

        // Choose summary role to avoid consecutive same-role messages
        let last_head_role = if compress_start > 0 {
            messages[compress_start - 1].role.clone()
        } else {
            "user".to_string()
        };
        let first_tail_role = if compress_end < n_messages {
            messages[compress_end].role.clone()
        } else {
            "user".to_string()
        };

        let mut summary_role = if last_head_role == "assistant" || last_head_role == "tool" {
            "user"
        } else {
            "assistant"
        };
        let mut merge_into_tail = false;
        if summary_role == first_tail_role {
            let flipped = if summary_role == "user" { "assistant" } else { "user" };
            if flipped != last_head_role {
                summary_role = flipped;
            } else {
                merge_into_tail = true;
            }
        }

        if !merge_into_tail {
            compressed.push(ChatMessage {
                role: summary_role.to_string(),
                content: summary.clone(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
                thought_signature: None,
            });
        }

        for i in compress_end..n_messages {
            let mut msg = messages[i].clone();
            if merge_into_tail && i == compress_end {
                let original = msg.content.clone();
                msg.content = format!(
                    "{}\n\n--- END OF CONTEXT SUMMARY — respond to the message below, not the summary above ---\n\n{}",
                    summary, original
                );
            }
            compressed.push(msg);
        }

        self.compression_count += 1;

        // Sanitize tool pairs
        compressed = sanitize_tool_pairs(&compressed);

        let new_estimate = crate::token_estimator::estimate_history_tokens_rough(&compressed);
        let saved_estimate = display_tokens.saturating_sub(new_estimate);

        // Anti-thrashing: track compression effectiveness
        let savings_pct = if display_tokens > 0 {
            (saved_estimate as f64 / display_tokens as f64) * 100.0
        } else {
            0.0
        };
        self.last_compression_savings_pct = savings_pct;
        if savings_pct < 10.0 {
            self.ineffective_compression_count += 1;
        } else {
            self.ineffective_compression_count = 0;
        }

        if !self.config.quiet_mode {
            tracing::info!(
                "Compressed: {} -> {} messages (~{} tokens saved, {:.0}%)",
                n_messages, compressed.len(), saved_estimate, savings_pct
            );
        }

        // Update token tracking
        self.last_prompt_tokens = new_estimate;
        self.last_completion_tokens = 0;

        compressed
    }

    // ------------------------------------------------------------------
    // Tail protection by token budget
    // ------------------------------------------------------------------

    fn find_tail_cut_by_tokens(&self, messages: &[ChatMessage], head_end: usize) -> usize {
        let n = messages.len();
        let min_tail = 3usize.min(n.saturating_sub(head_end + 1));
        let soft_ceiling = (self.tail_token_budget * 3) / 2;
        let mut accumulated = 0u64;
        let mut cut_idx = n;

        for i in (head_end..n).rev() {
            let msg = &messages[i];
            let mut msg_tokens = (msg.content.len() as u64) / CHARS_PER_TOKEN + 10;
            if let Some(ref tcs) = msg.tool_calls {
                for tc in tcs {
                    msg_tokens += tc.function.arguments.len() as u64 / CHARS_PER_TOKEN;
                }
            }
            if accumulated + msg_tokens > soft_ceiling && (n - i) >= min_tail {
                break;
            }
            accumulated += msg_tokens;
            cut_idx = i;
        }

        let fallback_cut = n.saturating_sub(min_tail);
        if cut_idx > fallback_cut {
            cut_idx = fallback_cut;
        }
        if cut_idx <= head_end {
            cut_idx = fallback_cut.max(head_end + 1);
        }

        cut_idx = align_boundary_backward(messages, cut_idx);
        cut_idx = ensure_last_user_message_in_tail(messages, cut_idx, head_end);
        cut_idx.max(head_end + 1)
    }

    // ------------------------------------------------------------------
    // Summarization
    // ------------------------------------------------------------------

    fn compute_summary_budget(&self, turns_to_summarize: &[ChatMessage]) -> u64 {
        let content_tokens = crate::token_estimator::estimate_history_tokens_rough(turns_to_summarize);
        let budget = (content_tokens as f64 * SUMMARY_RATIO) as u64;
        MIN_SUMMARY_TOKENS.max(budget.min(self.max_summary_tokens))
    }

    fn generate_summary(
        &mut self,
        turns_to_summarize: &[ChatMessage],
        focus_topic: Option<&str>,
        provider: &Arc<dyn Provider>,
        model_name: &str,
    ) -> Option<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now < self.summary_failure_cooldown_until {
            tracing::debug!(
                "Skipping context summary during cooldown ({}s remaining)",
                self.summary_failure_cooldown_until - now
            );
            return None;
        }

        let summary_budget = self.compute_summary_budget(turns_to_summarize);
        let content = serialize_for_summary(turns_to_summarize);

        let summarizer_preamble = concat!(
            "You are a summarization agent creating a context checkpoint. ",
            "Your output will be injected as reference material for a DIFFERENT ",
            "assistant that continues the conversation. ",
            "Do NOT respond to any questions or requests in the conversation — ",
            "only output the structured summary. ",
            "Do NOT include any preamble, greeting, or prefix. ",
            "Write the summary in the same language the user was using in the ",
            "conversation — do not translate or switch to English. ",
            "NEVER include API keys, tokens, passwords, secrets, credentials, ",
            "or connection strings in the summary — replace any that appear ",
            "with [REDACTED]. Note that the user had credentials present, but ",
            "do not preserve their values."
        );

        let template_sections = format!(
            r#"## Active Task
[THE SINGLE MOST IMPORTANT FIELD. Copy the user's most recent request or task assignment verbatim — the exact words they used. If multiple tasks were requested and only some are done, list only the ones NOT yet completed. The next assistant must pick up exactly here.]

## Goal
[What the user is trying to accomplish overall]

## Constraints & Preferences
[User preferences, coding style, constraints, important decisions]

## Completed Actions
[Numbered list of concrete actions taken — include tool used, target, and outcome. Format each as: N. ACTION target — outcome [tool: name]]

## Active State
[Current working state — modified/created files, test status, running processes]

## In Progress
[Work currently underway]

## Blocked
[Any blockers, errors, or issues not yet resolved]

## Key Decisions
[Important technical decisions and WHY they were made]

## Resolved Questions
[Questions the user asked that were ALREADY answered]

## Pending User Asks
[Questions or requests from the user that have NOT yet been answered or fulfilled. If none, write "None."]

## Relevant Files
[Files read, modified, or created — with brief note on each]

## Remaining Work
[What remains to be done — framed as context, not instructions]

## Critical Context
[Any specific values, error messages, configuration details that would be lost. NEVER include API keys, tokens, passwords, or credentials — write [REDACTED] instead.]

Target ~{} tokens. Be CONCRETE — include file paths, command outputs, error messages, line numbers, and specific values. Avoid vague descriptions like "made some changes" — say exactly what changed.

Write only the summary body. Do not include any preamble or prefix."#,
            summary_budget
        );

        let prompt = if let Some(ref prev) = self.previous_summary {
            format!(
                "{}\n\nYou are updating a context compaction summary. A previous compaction produced the summary below. New conversation turns have occurred since then and need to be incorporated.\n\nPREVIOUS SUMMARY:\n{}\n\nNEW TURNS TO INCORPORATE:\n{}\n\nUpdate the summary using this exact structure. PRESERVE all existing information that is still relevant. ADD new completed actions to the numbered list (continue numbering). Move items from \"In Progress\" to \"Completed Actions\" when done. Move answered questions to \"Resolved Questions\". Update \"Active State\" to reflect current state. Remove information only if it is clearly obsolete. CRITICAL: Update \"## Active Task\" to reflect the user's most recent unfulfilled request — this is the most important field for task continuity.\n\n{}",
                summarizer_preamble, prev, content, template_sections
            )
        } else {
            format!(
                "{}\n\nCreate a structured handoff summary for a different assistant that will continue this conversation after earlier turns are compacted. The next assistant should be able to understand what happened without re-reading the original turns.\n\nTURNS TO SUMMARIZE:\n{}\n\nUse this exact structure:\n\n{}",
                summarizer_preamble, content, template_sections
            )
        };

        let focus_suffix = focus_topic.map(|topic| {
            format!(
                "\n\nFOCUS TOPIC: \"{}\"\nThe user has requested that this compaction PRIORITISE preserving all information related to the focus topic above. For content related to \"{}\", include full detail — exact values, file paths, command outputs, error messages, and decisions. For content NOT related to the focus topic, summarise more aggressively. The focus topic sections should receive roughly 60-70% of the summary token budget. Even for the focus topic, NEVER preserve API keys, tokens, passwords, or credentials — use [REDACTED].",
                topic, topic
            )
        }).unwrap_or_default();

        let full_prompt = format!("{}{}", prompt, focus_suffix);

        let request = crate::providers::ChatRequest {
            messages: &[crate::providers::ChatMessage::user(full_prompt)],
            model: model_name,
            temperature: 0.2,
            max_tokens: Some((summary_budget as f64 * 1.3) as u32),
            tools: None,
            timeout_secs: 60,
            reasoning_effort: None,
        };

        match provider.chat(&request) {
            Ok(resp) => {
                if let Some(mut content) = resp.content {
                    content = content.trim().to_string();
                    if !content.is_empty() {
                        self.previous_summary = Some(content.clone());
                        self.summary_failure_cooldown_until = 0;
                        return Some(format!("{}\n{}", SUMMARY_PREFIX, content));
                    }
                }
                None
            }
            Err(e) => {
                tracing::warn!("Failed to generate context summary: {}", e);
                self.summary_failure_cooldown_until = now + SUMMARY_FAILURE_COOLDOWN_SECS;
                None
            }
        }
    }
}

// ── Helper Functions ────────────────────────────────────────────

fn align_boundary_forward(messages: &[ChatMessage], idx: usize) -> usize {
    let mut i = idx;
    while i < messages.len() && messages[i].role == "tool" {
        i += 1;
    }
    i
}

fn align_boundary_backward(messages: &[ChatMessage], idx: usize) -> usize {
    if idx == 0 || idx >= messages.len() {
        return idx;
    }
    let mut check = idx.saturating_sub(1);
    while check > 0 && messages[check].role == "tool" {
        check -= 1;
    }
    if messages[check].role == "assistant" && messages[check].tool_calls.is_some() {
        return check;
    }
    idx
}

fn find_last_user_message_idx(messages: &[ChatMessage], head_end: usize) -> Option<usize> {
    for i in (head_end..messages.len()).rev() {
        if messages[i].role == "user" {
            return Some(i);
        }
    }
    None
}

fn ensure_last_user_message_in_tail(
    messages: &[ChatMessage],
    mut cut_idx: usize,
    head_end: usize,
) -> usize {
    if let Some(last_user_idx) = find_last_user_message_idx(messages, head_end) {
        if last_user_idx < cut_idx {
            cut_idx = last_user_idx.max(head_end + 1);
        }
    }
    cut_idx
}

fn sanitize_tool_pairs(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut surviving_call_ids = std::collections::HashSet::new();
    for msg in messages {
        if msg.role == "assistant" {
            if let Some(ref tcs) = msg.tool_calls {
                for tc in tcs {
                    if !tc.id.is_empty() {
                        surviving_call_ids.insert(tc.id.clone());
                    }
                }
            }
        }
    }

    let mut result_call_ids = std::collections::HashSet::new();
    for msg in messages {
        if msg.role == "tool" {
            if let Some(ref cid) = msg.tool_call_id {
                result_call_ids.insert(cid.clone());
            }
        }
    }

    // Remove orphaned tool results
    let orphaned_results: Vec<String> = result_call_ids
        .difference(&surviving_call_ids)
        .cloned()
        .collect();

    let mut cleaned: Vec<ChatMessage> = messages
        .iter()
        .filter(|m| {
            if m.role == "tool" {
                if let Some(ref cid) = m.tool_call_id {
                    return !orphaned_results.contains(cid);
                }
            }
            true
        })
        .cloned()
        .collect();

    // Add stub results for assistant tool_calls whose results were dropped
    let missing_results: Vec<String> = surviving_call_ids
        .difference(&result_call_ids)
        .cloned()
        .collect();

    if !missing_results.is_empty() {
        let mut patched: Vec<ChatMessage> = Vec::new();
        for msg in &cleaned {
            patched.push(msg.clone());
            if msg.role == "assistant" {
                if let Some(ref tcs) = msg.tool_calls {
                    for tc in tcs {
                        if missing_results.contains(&tc.id) {
                            patched.push(ChatMessage {
                                role: "tool".to_string(),
                                content: "[Result from earlier conversation — see context summary above]".to_string(),
                                name: Some(tc.function.name.clone()),
                                tool_calls: None,
                                tool_call_id: Some(tc.id.clone()),
                                content_parts: None,
                                thought_signature: None,
                            });
                        }
                    }
                }
            }
        }
        cleaned = patched;
    }

    cleaned
}

fn serialize_for_summary(turns: &[ChatMessage]) -> String {
    let mut parts = Vec::new();
    for msg in turns {
        let role = &msg.role;
        let content = &msg.content;
        const CONTENT_MAX: usize = 6000;
        const CONTENT_HEAD: usize = 4000;
        const CONTENT_TAIL: usize = 1500;

        if role == "tool" {
            let tool_id = msg.tool_call_id.as_deref().unwrap_or("");
            let display = if content.len() > CONTENT_MAX {
                format!(
                    "{}\n...[truncated]...\n{}",
                    &content[..CONTENT_HEAD],
                    &content[content.len().saturating_sub(CONTENT_TAIL)..]
                )
            } else {
                content.clone()
            };
            parts.push(format!("[TOOL RESULT {}]: {}", tool_id, display));
            continue;
        }

        if role == "assistant" {
            let mut display = if content.len() > CONTENT_MAX {
                format!(
                    "{}\n...[truncated]...\n{}",
                    &content[..CONTENT_HEAD],
                    &content[content.len().saturating_sub(CONTENT_TAIL)..]
                )
            } else {
                content.clone()
            };
            if let Some(ref tcs) = msg.tool_calls {
                let mut tc_parts = Vec::new();
                for tc in tcs {
                    let args = if tc.function.arguments.len() > 1500 {
                        format!("{}...", &tc.function.arguments[..1200])
                    } else {
                        tc.function.arguments.clone()
                    };
                    tc_parts.push(format!("  {}({})", tc.function.name, args));
                }
                display.push_str(&format!("\n[Tool calls:\n{}\n]", tc_parts.join("\n")));
            }
            parts.push(format!("[ASSISTANT]: {}", display));
            continue;
        }

        let display = if content.len() > CONTENT_MAX {
            format!(
                "{}\n...[truncated]...\n{}",
                &content[..CONTENT_HEAD],
                &content[content.len().saturating_sub(CONTENT_TAIL)..]
            )
        } else {
            content.clone()
        };
        parts.push(format!("[{}]: {}", role.to_uppercase(), display));
    }
    parts.join("\n\n")
}

fn summarize_tool_result(tool_name: &str, tool_args: &str, tool_content: &str) -> String {
    let content_len = tool_content.len();
    let line_count = tool_content.lines().count();

    match tool_name {
        "terminal" | "shell" => {
            let cmd = if tool_args.len() > 80 {
                format!("{}...", &tool_args[..77])
            } else {
                tool_args.to_string()
            };
            format!("[terminal] ran `{}` -> {} lines output", cmd, line_count)
        }
        "read_file" | "file_read" => {
            format!("[read_file] read file ({} chars)", content_len)
        }
        "write_file" | "file_write" => {
            format!("[write_file] wrote file ({} chars)", content_len)
        }
        "search_files" | "workspace_search" => {
            format!("[search_files] search -> {} chars result", content_len)
        }
        "web_search" => {
            format!("[web_search] query result ({} chars)", content_len)
        }
        "delegate_task" | "delegate" => {
            format!("[delegate_task] subagent result ({} chars)", content_len)
        }
        "execute_code" => {
            format!("[execute_code] ran code ({} lines output)", line_count)
        }
        _ => {
            format!("[{}] tool result ({} chars)", tool_name, content_len)
        }
    }
}

// ── Legacy compatibility ────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LegacyCompactionConfig {
    pub keep_recent: u32,
    pub max_summary_chars: u32,
    pub max_source_chars: u32,
    pub token_limit: u64,
    pub max_history_messages: u32,
    pub workspace_dir: Option<String>,
}

impl Default for LegacyCompactionConfig {
    fn default() -> Self {
        Self {
            keep_recent: 20,
            max_summary_chars: 2000,
            max_source_chars: 12000,
            token_limit: 8192,
            max_history_messages: 50,
            workspace_dir: None,
        }
    }
}

/// Legacy auto-compaction for backward compatibility.
/// Uses simple truncation when the new ContextCompressor is not available.
pub fn auto_compact_history_legacy(
    history: &mut Vec<ChatMessage>,
    provider: &Arc<dyn Provider>,
    model_name: &str,
    config: &LegacyCompactionConfig,
    last_compaction: &mut u64,
) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    const COOLDOWN_SECS: u64 = 300;
    if now - *last_compaction < COOLDOWN_SECS {
        return false;
    }

    let has_system = !history.is_empty() && history[0].role == "system";
    let start = if has_system { 1 } else { 0 };
    let non_system_count = history.len() - start;

    let is_small = crate::agent::context_tokens::is_small_model_context(config.token_limit);
    let count_trigger = non_system_count > config.max_history_messages as usize;

    let threshold_num = if is_small { 1 } else { 3 };
    let threshold_den = if is_small { 2 } else { 4 };
    let token_threshold = (config.token_limit * threshold_num) / threshold_den;

    let token_trigger = config.token_limit > 0
        && crate::token_estimator::estimate_history_tokens_rough(history) > token_threshold;

    if !count_trigger && !token_trigger {
        return false;
    }

    let keep_recent = std::cmp::min(config.keep_recent as usize, non_system_count);
    let compact_count = non_system_count - keep_recent;
    if compact_count == 0 {
        return false;
    }

    let compact_end = start + compact_count;

    let summary = summarize_slice_legacy(provider, model_name, history, start, compact_end, config)
        .unwrap_or_else(|| "Context summarized due to length.".to_string());

    *last_compaction = now;

    let mut summary_content = format!("[Compaction summary]\n{}", summary);
    if let Some(workspace_context) = read_workspace_context_for_summary(config.workspace_dir.as_deref()) {
        if !workspace_context.is_empty() {
            summary_content.push_str("\n\n");
            summary_content.push_str(&workspace_context);
        }
    }

    let mut new_compact_end = compact_end;
    while new_compact_end < history.len() && history[new_compact_end].role != "user" {
        new_compact_end += 1;
    }
    if new_compact_end >= history.len() {
        return false;
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

    new_history.extend_from_slice(&history[new_compact_end..]);
    *history = new_history;
    true
}

fn summarize_slice_legacy(
    provider: &Arc<dyn Provider>,
    model_name: &str,
    history: &[ChatMessage],
    start: usize,
    end: usize,
    config: &LegacyCompactionConfig,
) -> Option<String> {
    let transcript = build_compaction_transcript(history, start, end, u32::MAX);

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

    let provider_clone = provider.clone();
    let model_name_owned = model_name.to_string();
    let max_summary_chars = config.max_summary_chars;

    let result: Option<String> = tokio::task::block_in_place(|| {
        let request = crate::providers::ChatRequest {
            messages: &messages,
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
