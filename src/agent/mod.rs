pub mod background_review;
pub mod commands;
pub mod compaction;
pub mod context_tokens;
pub mod dispatcher;
pub mod max_tokens;
pub mod memory_loader;
pub mod prompt;
pub mod prompt_cache;
pub mod response_cache;
pub mod routing;

pub use routing::{
    AgentBinding, BindingMatch, ChatType, MatchedBy, PeerRef, ResolvedRoute, RouteInput,
    resolve_route,
};

use anyhow::Result;
use regex::Regex;
use std::sync::Arc;
use std::sync::OnceLock;
use tracing::warn;

use crate::model_router::ModelRoutingConfig;
use crate::providers::{ChatMessage, ChatRequest, Provider, ToolSpec};
use crate::tools::{Tool, ToolContext};

use compaction::{auto_compact_history_legacy, trim_history};
use context_tokens as context_tokens_resolver;
use dispatcher::{ParsedToolCall, ToolExecutionResult, parse_tool_calls};
use max_tokens as max_tokens_resolver;
use memory_loader::{Memory, NoopMemory};

const MAX_SAME_TOOL_RETRIES: u32 = 3;

pub struct Agent {
    pub provider: Arc<dyn Provider>,
    pub cheap_provider: Option<Arc<dyn Provider>>,
    pub tools: Vec<Arc<dyn Tool>>,
    pub memory: Arc<dyn Memory>,
    pub model_name: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub token_limit: u64,
    pub max_tool_iterations: u32,
    pub max_history_messages: usize,
    pub workspace_dir: String,
    pub config_path: Option<String>,
    pub auto_save: bool,
    pub has_system_prompt: bool,
    pub workspace_prompt_fingerprint: Option<u64>,
    pub memory_session_id: Option<String>,
    pub history: Vec<ChatMessage>,
    pub total_tokens: u64,
    pub tool_cache: crate::tools::cache::ToolCache,
    pub command_registry: commands::CommandRegistry,
    pub model_routing_config: ModelRoutingConfig,
    pub cached_system_prompt: OnceLock<String>,
    pub last_tool_hash: u64,
    pub cost_tracker: Option<crate::cost::CostTracker>,
    pub response_cache: response_cache::ResponseCache,
    pub multimodal_config: crate::multimodal::MultimodalConfig,
    /// Per-agent compaction cooldown timestamp (unix secs). Replaces the old
    /// process-global AtomicU64 so multiple concurrent agents don’t block each other.
    pub last_compaction: u64,
    /// Channel/sender context injected per-turn so the system prompt can activate
    /// group-chat logic, channel-specific behaviour etc. (BUG-5 fix)
    pub conversation_context: Option<prompt::ConversationContext>,

    // Self-Learning counters (Hermes-style)
    pub user_turn_count: u64,
    pub iters_since_skill: u32,
    pub turns_since_memory: u32,
    pub skill_nudge_interval: u32,
    pub memory_nudge_interval: u32,
    pub memory_flush_min_turns: u32,

    // Frozen memory snapshots (Hermes-style prefix-cache preservation)
    pub memory_snapshot: Option<String>,
    pub user_snapshot: Option<String>,

    // Efficiency features (Hermes-style cost/token optimization)
    pub skill_journal: Option<Arc<crate::skills::self_improve::SkillJournal>>,
    pub efficiency_config: crate::config_types::EfficiencyConfig,
    pub current_turn_tokens: u64,

    // Hermes-style ContextCompressor for advanced context compression
    pub context_compressor: Option<crate::agent::compaction::ContextCompressor>,
}

/// QW-6: Conditional follow-through detection.
/// Returns true if the LLM output contains phrases indicating it intends to take action,
/// but didn't actually emit a tool call.
/// Select a subset of history for the current turn.
/// For trivial follow-ups, keep only recent messages to save tokens.
/// Always preserves the system prompt at index 0.
fn select_context_window(history: &[ChatMessage], query: &str) -> Vec<ChatMessage> {
    if history.is_empty() {
        return history.to_vec();
    }

    let lower = query.to_lowercase();
    let trimmed = lower.trim();

    // Trivial follow-ups need very little prior context
    let is_trivial_followup = trimmed == "yes"
        || trimmed == "no"
        || trimmed == "ok"
        || trimmed == "thanks"
        || trimmed == "thank you"
        || trimmed.starts_with("yes ")
        || trimmed.starts_with("no ")
        || trimmed.starts_with("ok ")
        || trimmed.starts_with("thanks ")
        || trimmed.starts_with("thank you ");

    if is_trivial_followup && history.len() > 8 {
        let mut sliced = Vec::new();
        if history[0].role == "system" {
            sliced.push(history[0].clone());
        }
        let start = history.len().saturating_sub(6);
        sliced.extend_from_slice(&history[start..]);
        return sliced;
    }

    // New topic markers: user explicitly wants full context
    let is_new_topic = lower.starts_with("actually ")
        || lower.starts_with("new topic")
        || lower.starts_with("forget ")
        || lower.starts_with("let's talk about ");

    if is_new_topic {
        return history.to_vec();
    }

    history.to_vec()
}

pub fn should_force_follow_through(text: &str, available_tools: &[Arc<dyn Tool>]) -> bool {
    if available_tools.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    let trimmed = text.trim();

    // 0. Safe phrases that should NEVER trigger follow-through
    const SAFE_PHRASES: &[&str] = &["let me know", "i'll let you know", "i will let you know"];
    if SAFE_PHRASES.iter().any(|p| lower.contains(p)) {
        return false;
    }

    // Intent starters
    const STARTERS: &[&str] = &[
        "let me",
        "i will",
        "i'll",
        "let's",
        "i'm going to",
        "i am going to",
        "i need to",
        "i should",
        "gonna",
        "so i need",
    ];
    // Action verbs
    const VERBS: &[&str] = &[
        "try",
        "check",
        "run",
        "do",
        "get",
        "execute",
        "look",
        "apply",
        "fix",
        "start",
        "use",
        "search",
        "fetch",
        "read",
        "write",
        "list",
        "ls",
        "edit",
        "update",
        "modify",
        "patch",
        "delete",
        "remove",
        "create",
        "make",
        "find",
        "investigate",
        "analyze",
        "explore",
        "install",
    ];
    // Immediacy indicators
    const IMMEDIACY: &[&str] = &[
        "now",
        "again",
        "immediately",
        "straight away",
        "instead",
        "briefly",
    ];

    let has_starter = STARTERS.iter().any(|s| lower.contains(s));

    // Improved verb check: avoid matching "ls" in "else" or "do" in "down"
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|s| !s.is_empty())
        .collect();

    let has_verb = VERBS.iter().any(|&v| {
        if v.len() <= 2 {
            words.contains(&v)
        } else {
            lower.contains(v)
        }
    });

    let has_immediacy = IMMEDIACY.iter().any(|a| lower.contains(a));
    let ends_with_colon = trimmed.ends_with(':') || trimmed.ends_with("...");

    // 1. High confidence trigger: (Starter + Verb OR Verb-ing) + (Immediacy OR Colon/Ellipsis)
    let is_acting_present = lower.starts_with("checking")
        || lower.starts_with("investigating")
        || lower.starts_with("searching")
        || lower.starts_with("analyzing")
        || lower.starts_with("installing");
    if (has_starter && has_verb && (has_immediacy || ends_with_colon))
        || (is_acting_present && (has_immediacy || ends_with_colon))
    {
        return true;
    }

    // 2. Phrase-based fallback
    const PHRASES: &[&str] = &["starting the", "applying the", "let's see what i can do"];
    if PHRASES.iter().any(|p| lower.contains(p)) {
        return true;
    }

    // 3. Tool Relevance Trigger
    if has_starter && has_verb {
        let has_relevant_tool = available_tools.iter().any(|t| {
            let n = t.name().to_lowercase();
            n.contains("read")
                || n.contains("fetch")
                || n.contains("search")
                || n.contains("ls")
                || n.contains("get")
                || n.contains("shell")
                || n.contains("terminal")
                || n.contains("browser")
                || n.contains("file")
                || n.contains("write")
                || n.contains("edit")
                || n.contains("run")
                || n.contains("exec")
                || n.contains("git")
                || n.contains("tool")
        });

        if has_relevant_tool && let Some(last_line) = trimmed.lines().last() {
            let last_lower = last_line.to_lowercase();
            let last_words: Vec<&str> = last_lower
                .split(|c: char| !c.is_alphanumeric() && c != '\'')
                .filter(|s| !s.is_empty())
                .collect();

            let last_has_starter = STARTERS.iter().any(|s| last_lower.contains(s));
            let last_has_verb = VERBS.iter().any(|&v| {
                if v.len() <= 2 {
                    last_words.contains(&v)
                } else {
                    last_lower.contains(v)
                }
            });

            if last_has_starter && last_has_verb {
                return true;
            }
        }
    }

    false
}

/// Removes raw tool-call markup from LLM output that leaked into the final text.
/// Strips:
///   - `<tool_call>...</tool_call>` XML blocks
///   - `[tool_call]...[/tool_call]` and `[TOOL_CALL]...[/TOOL_CALL]` bracket blocks
///   - `` ```tool_call...``` `` fenced code blocks
///   - `` ```tool_code...``` `` fenced blocks that look like Python/JSON dumps
///     Returns a trimmed result; may be empty if everything was stripped.
pub fn strip_tool_call_markup(text: &str) -> String {
    use std::sync::OnceLock;

    static XML_RE: OnceLock<Regex> = OnceLock::new();
    static BRACKET_RE: OnceLock<Regex> = OnceLock::new();
    static FENCED_TC_RE: OnceLock<Regex> = OnceLock::new();
    static FENCED_PY_RE: OnceLock<Regex> = OnceLock::new();
    static FENCED_PY2_RE: OnceLock<Regex> = OnceLock::new();

    let xml_re = XML_RE.get_or_init(|| Regex::new(r"(?si)<tool_call>.*?</tool_call>").unwrap());
    let out = xml_re.replace_all(text, "");

    let bracket_re =
        BRACKET_RE.get_or_init(|| Regex::new(r"(?si)\[tool_call\].*?\[/tool_call\]").unwrap());
    let out = bracket_re.replace_all(&out, "");

    let fenced_tc_re =
        FENCED_TC_RE.get_or_init(|| Regex::new(r"(?si)```\s*tool_call.*?```").unwrap());
    let out = fenced_tc_re.replace_all(&out, "");

    let fenced_py_re = FENCED_PY_RE
        .get_or_init(|| Regex::new(r"(?si)```\s*tool_code\s*\nimport json.*?```").unwrap());
    let out = fenced_py_re.replace_all(&out, "");

    let fenced_py2_re = FENCED_PY2_RE
        .get_or_init(|| Regex::new(r"(?si)```\s*tool_code\s*\njson\.loads\(.*?```").unwrap());
    let out = fenced_py2_re.replace_all(&out, "");

    let trimmed = out.trim().to_string();
    trimmed
}

/// Checks if a response is "low-value" or a placeholder (e.g. "✅ Done.", "ok", etc.)
/// that should be replaced by a proper summary if tools were called.
pub fn is_low_value_response(text: &str) -> bool {
    let lower = text.to_lowercase();
    let trimmed = lower.trim();

    if trimmed.is_empty() {
        return true;
    }

    // Common placeholders
    const PLACEHOLDERS: &[&str] = &[
        "✅ done.",
        "done.",
        "ok.",
        "task complete.",
        "finished.",
        "ready.",
        "all done.",
        "fixed.",
        "submitted.",
    ];

    if PLACEHOLDERS.iter().any(|&p| trimmed == p) {
        return true;
    }

    // Only punctuation or symbols
    if trimmed
        .chars()
        .all(|c| c.is_ascii_punctuation() || !c.is_alphanumeric())
    {
        return true;
    }

    false
}

impl Agent {
    pub fn new(
        provider: Arc<dyn Provider>,
        cheap_provider: Option<Arc<dyn Provider>>,
        tools: Vec<Arc<dyn Tool>>,
        model_name: String,
        workspace_dir: String,
    ) -> Self {
        // Resolve token limits using new modules
        let token_limit = context_tokens_resolver::resolve_context_tokens(None, &model_name);
        let max_tokens = max_tokens_resolver::resolve_max_tokens(None, &model_name);

        let mut agent = Self {
            provider,
            cheap_provider,
            tools,
            memory: Arc::new(NoopMemory), // Default to no-op
            model_name: model_name.clone(),
            temperature: 0.7,
            max_tokens,
            token_limit,
            max_tool_iterations: 40,
            max_history_messages: 50,
            workspace_dir: workspace_dir.clone(),
            config_path: None, // Will be set by set_config_path if available
            auto_save: true,
            has_system_prompt: false,
            workspace_prompt_fingerprint: None,
            memory_session_id: None,
            history: Vec::new(),
            total_tokens: 0,
            tool_cache: crate::tools::cache::ToolCache::new(300), // Default 5 min TTL
            command_registry: commands::CommandRegistry::new(),
            model_routing_config: ModelRoutingConfig {
                cheap_model: model_name.clone(), // Default to same, will be tuned by config
                default_model: model_name,
            },
            cached_system_prompt: OnceLock::new(),
            last_tool_hash: 0,
            cost_tracker: Some(crate::cost::CostTracker::init(
                &workspace_dir,
                true,
                10.0,
                100.0,
                80,
            )),
            response_cache: response_cache::ResponseCache::new(3600), // 1 hour TTL
            multimodal_config: crate::multimodal::MultimodalConfig {
                allowed_dirs: vec![
                    workspace_dir.clone(),
                    std::env::temp_dir().to_string_lossy().to_string(),
                ],
                ..Default::default()
            },
            last_compaction: 0,
            conversation_context: None,
            user_turn_count: 0,
            iters_since_skill: 0,
            turns_since_memory: 0,
            skill_nudge_interval: 15,
            memory_nudge_interval: 10,
            memory_flush_min_turns: 6,
            memory_snapshot: None,
            user_snapshot: None,
            skill_journal: None,
            efficiency_config: crate::config_types::EfficiencyConfig::default(),
            current_turn_tokens: 0,
            context_compressor: Some(crate::agent::compaction::ContextCompressor::new(
                crate::agent::compaction::CompactionConfig {
                    context_length: token_limit,
                    ..Default::default()
                },
            )),
        };

        if context_tokens_resolver::is_small_model_context(token_limit) {
            agent.max_tool_iterations = 150; // Increased for local models
        }

        agent
    }

    pub fn with_memory(mut self, memory: Arc<dyn Memory>) -> Self {
        self.memory = memory;
        self
    }

    pub fn with_system_prompt(mut self, prompt: &str) -> Self {
        let _ = self.cached_system_prompt.set(prompt.to_string());
        self.has_system_prompt = true;
        self
    }

    pub fn reset_history(&mut self) {
        self.history.clear();
        self.has_system_prompt = false;
        self.workspace_prompt_fingerprint = None;
        self.cached_system_prompt = OnceLock::new();
        self.last_tool_hash = 0;
        self.conversation_context = None;
    }

    fn parse_multimodal_message(
        &self,
        content: &str,
    ) -> (String, Option<Vec<crate::providers::ContentPart>>) {
        let parse_result = crate::multimodal::parse_multimodal_markers(content);
        if parse_result.refs.is_empty() {
            return (content.to_string(), None);
        }

        let mut parts = Vec::new();
        parts.push(crate::providers::ContentPart::Text(
            parse_result.cleaned_text.clone(),
        ));

        for m_ref in parse_result.refs {
            let ref_path = m_ref.path;
            match crate::multimodal::read_local_file(&ref_path, &self.multimodal_config) {
                Ok(data) => {
                    parts.push(crate::providers::ContentPart::Media {
                        mime_type: data.mime_type,
                        data: data.data,
                    });
                }
                Err(e) => {
                    warn!("Failed to read multimodal file {}: {}", ref_path, e);
                    // Add a placeholder text so the model knows a file was intended but failed
                    parts.push(crate::providers::ContentPart::Text(format!(
                        "\n[Error loading file: {}]",
                        ref_path
                    )));
                }
            }
        }

        (parse_result.cleaned_text, Some(parts))
    }

    fn compute_tool_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        for tool in &self.tools {
            tool.name().hash(&mut hasher);
            tool.parameters_json().hash(&mut hasher);
        }
        hasher.finish()
    }

    pub fn build_system_prompt_cached(&mut self) -> Result<String> {
        let current_tool_hash = self.compute_tool_hash();
        let workspace_fp = prompt::workspace_prompt_fingerprint(&self.workspace_dir);

        if let Some(cached) = self.cached_system_prompt.get() {
            if current_tool_hash == self.last_tool_hash
                && Some(workspace_fp) == self.workspace_prompt_fingerprint
            {
                return Ok(cached.clone());
            }
        }

        let prompt = self.build_system_prompt()?;
        self.cached_system_prompt = OnceLock::new();
        let _ = self.cached_system_prompt.set(prompt.clone());
        self.last_tool_hash = current_tool_hash;
        self.workspace_prompt_fingerprint = Some(workspace_fp);
        Ok(prompt)
    }

    pub fn build_system_prompt(&mut self) -> Result<String> {
        let caps = None; // Capabilities not yet implemented
        let conversation_context = self.conversation_context.as_ref(); 

        let learnings = self.memory
            .list_with_category(Some(crate::memory::MemoryCategory::Learning), self.memory_session_id.as_deref())
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.content)
            .collect::<Vec<_>>();

        // Hermes-style frozen snapshot: capture memory files at prompt-build time.
        // Mid-session memory tool writes update disk but do NOT mutate these snapshots
        // until the workspace fingerprint changes (triggering a prompt rebuild),
        // preserving the LLM's prefix cache for the system prompt.
        let mem_path = std::path::Path::new(&self.workspace_dir).join("MEMORY.md");
        let alt_path = std::path::Path::new(&self.workspace_dir).join("memory.md");
        self.memory_snapshot = std::fs::read_to_string(&mem_path)
            .ok()
            .or_else(|| std::fs::read_to_string(&alt_path).ok())
            .filter(|s| !s.trim().is_empty());

        let user_path = std::path::Path::new(&self.workspace_dir).join("USER.md");
        self.user_snapshot = std::fs::read_to_string(&user_path)
            .ok()
            .filter(|s| !s.trim().is_empty());

        let ctx = prompt::PromptContext {
            workspace_dir: &self.workspace_dir,
            model_name: &self.model_name,
            tools: &self.tools,
            capabilities_section: caps,
            conversation_context,
            use_native_tools: self.provider.supports_native_tools(),
            token_limit: self.token_limit,
            learnings,
            memory_snapshot: self.memory_snapshot.clone(),
            user_snapshot: self.user_snapshot.clone(),
        };

        Ok(prompt::build_system_prompt(ctx))
    }

    pub async fn turn(&mut self, user_message: String, context: &ToolContext) -> Result<String> {
        use crate::model_router;

        // Handle Slash Commands
        if let Some(slash_response) = commands::CommandRegistry::handle_message(self, &user_message)
        {
            return Ok(slash_response);
        }

        // Self-Learning: Track user turns and memory nudge
        self.user_turn_count += 1;
        if self.memory_nudge_interval > 0 {
            self.turns_since_memory += 1;
        }

        // QW-7: Cheap model routing for greetings/simple tasks
        let model_to_use =
            model_router::route_to_appropriate_model(&user_message, &self.model_routing_config)
                .to_string();

        // Select the appropriate provider for the routed model
        let active_provider = if model_to_use == self.model_routing_config.cheap_model {
            self.cheap_provider.clone().unwrap_or_else(|| self.provider.clone())
        } else {
            self.provider.clone()
        };

        // R-1 & QW-2: Memory optimizations
        if self.auto_save && user_message.len() > 20 && !model_router::is_greeting(&user_message) {
            let key = format!(
                "autosave_user_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            );
            let _ = self
                .memory
                .store(&key, &user_message, self.memory_session_id.as_deref());
        }

        // R-3: System Prompt Management (Cached)
        if !self.has_system_prompt || self.cached_system_prompt.get().is_none() {
            let mut system_prompt = self.build_system_prompt_cached()?;

            // Always ensure the time is fresh in the prompt
            if let Some(start_idx) = system_prompt.find("## Current Date & Time") {
                let mut fresh_time = String::new();
                prompt::append_date_time_section(&mut fresh_time);

                let end_idx = system_prompt[start_idx + 22..]
                    .find("##")
                    .map(|i| i + start_idx + 22)
                    .unwrap_or(system_prompt.len());

                system_prompt.replace_range(start_idx..end_idx, &fresh_time);
            }

            // Insert or replace system prompt at index 0
            if !self.history.is_empty() && self.history[0].role == "system" {
                let mut new_content = system_prompt;
                if let Some(pos) = self.history[0]
                    .content
                    .find("\n\n--- COMPACTED PAST CONTEXT ---\n")
                {
                    new_content.push_str(&self.history[0].content[pos..]);
                }
                self.history[0].content = new_content;
            } else {
                self.history.insert(
                    0,
                    ChatMessage {
                        role: "system".to_string(),
                        content: system_prompt,
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                        content_parts: None,
                        thought_signature: None,
                    },
                );
            }
            self.has_system_prompt = true;
        } else if !self.history.is_empty() && self.history[0].role == "system" {
            // Even if the system prompt was set manually or is already present,
            // try to refresh the time section if it exists.
            let mut content = self.history[0].content.clone();
            if let Some(start_idx) = content.find("## Current Date & Time") {
                let mut fresh_time = String::new();
                prompt::append_date_time_section(&mut fresh_time);

                let end_idx = content[start_idx + 22..]
                    .find("##")
                    .map(|i| i + start_idx + 22)
                    .unwrap_or(content.len());

                content.replace_range(start_idx..end_idx, &fresh_time);
                self.history[0].content = content;
            }
        }

        // R-6 & QW-2: Enrich user message with memory context (only if substantive)
        let enriched_msg = if user_message.len() > 30
            && !model_router::is_greeting(&user_message)
            && self.memory.as_any().downcast_ref::<NoopMemory>().is_none()
        {
            match memory_loader::enrich_message(
                self.memory.as_ref(),
                &user_message,
                self.memory_session_id.as_deref(),
            ) {
                Ok(msg) => msg,
                Err(e) => {
                    warn!("Memory enrichment failed: {}", e);
                    user_message.clone()
                }
            }
        } else {
            user_message.clone()
        };

        // Hermes-style: inject dialectic user model into the user message (not system prompt)
        // to preserve the LLM's prefix cache.
        let dialectic = crate::dialectic::load_dialectic_context(&self.workspace_dir);
        let enriched_msg = if !dialectic.is_empty() {
            format!("<memory-context>\n{}\n</memory-context>\n\n{}", dialectic, enriched_msg)
        } else {
            enriched_msg
        };

        let (final_content, content_parts) = self.parse_multimodal_message(&enriched_msg);

        // Append user message
        self.history.push(ChatMessage {
            role: "user".to_string(),
            content: final_content,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts,
            thought_signature: None,
        });

        // Enforce history limits
        trim_history(&mut self.history, self.max_history_messages as u32);

        // Prepare ToolSpecs once
        let tool_specs: Vec<ToolSpec> = self
            .tools
            .iter()
            .map(|t| {
                let params: serde_json::Value =
                    serde_json::from_str(&t.parameters_json()).unwrap_or(serde_json::json!({}));
                ToolSpec {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: params,
                }
            })
            .collect();

        // ── HERMES: Pre-flight context compression ──────────────────
        // Before entering the main loop, check if loaded conversation history
        // already exceeds the model's context threshold. This handles cases
        // where a user switches to a model with a smaller context window
        // while having a large existing session.
        if let Some(ref mut compressor) = self.context_compressor {
            let min_for_compress = compressor.config.protect_first_n + 3 + 1;
            if self.history.len() > min_for_compress {
                let system_prompt = self.history.get(0)
                    .filter(|m| m.role == "system")
                    .map(|m| m.content.clone())
                    .unwrap_or_default();
                let preflight_tokens = crate::token_estimator::estimate_request_tokens_rough(
                    &self.history,
                    &system_prompt,
                    if tool_specs.is_empty() { None } else { Some(&tool_specs) },
                );
                if compressor.should_compress(preflight_tokens) {
                    tracing::info!(
                        "Preflight compression: ~{} tokens >= {} threshold",
                        preflight_tokens, compressor.threshold_tokens
                    );
                    let compaction_provider = self.cheap_provider.clone().unwrap_or_else(|| self.provider.clone());
                    for _pass in 0..3 {
                        let orig_len = self.history.len();
                        self.history = compressor.compress(
                            &mut self.history,
                            Some(preflight_tokens),
                            None,
                            &compaction_provider,
                            &self.model_routing_config.cheap_model,
                        );
                        if self.history.len() >= orig_len {
                            break;
                        }
                        let new_tokens = crate::token_estimator::estimate_request_tokens_rough(
                            &self.history,
                            &system_prompt,
                            if tool_specs.is_empty() { None } else { Some(&tool_specs) },
                        );
                        if new_tokens < compressor.threshold_tokens {
                            break;
                        }
                    }
                }
            }
        }

        // BUG-3 FIX: history_slice is built INSIDE the loop (see below).
        // The old short-message heuristic that sliced to 10 messages is removed —
        // it caused the model to lose all prior context on short follow-ups.

        // Tool loop
        let mut iterations = 0u32;
        let mut accumulated_text = String::new();
        // BUG-2 FIX: Independent counters for each nudge type so they cannot
        // compound into an infinite loop.
        let mut follow_through_nudge_count = 0u32;
        let mut empty_response_nudge_count = 0u32;
        // BUG-7 FIX: Track tool call repetitions across iterations to prevent
        // infinite retry loops (e.g. gemini CLI failing repeatedly).
        let mut tool_call_counts: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();

        loop {
            if iterations >= self.max_tool_iterations {
                warn!("Max tool iterations reached");
                // BUG-4 FIX: Clone the live history so the summary uses all current context.
                // We clone to satisfy Rust's borrow rules (&mut self + &self.history conflict).
                let history_for_summary = self.history.clone();
                let summary = self
                    .iterations_exhausted_summary(&history_for_summary, &model_to_use)
                    .await?;
                if !accumulated_text.is_empty() {
                    return Ok(format!("{}\n\n{}", accumulated_text.trim(), summary));
                }
                return Ok(summary);
            }

            // BUG-1 FIX: Rebuild fresh slice every iteration so the provider always
            // sees the latest tool results, assistant messages, and nudges.
            // CONTEXT SLICING: On the first iteration, slice trivial follow-ups
            // to save tokens; subsequent iterations need full history for tool results.
            let history_slice = if iterations == 0 {
                select_context_window(&self.history, &user_message)
            } else {
                self.history.clone()
            };

            // Resolve adaptive max tokens using fresh history length
            let mut current_max_tokens =
                max_tokens_resolver::resolve_max_tokens(Some(history_slice.len()), &model_to_use);

            // Enforce per-turn token budget from config
            if self.efficiency_config.turn_token_budget > 0 {
                current_max_tokens = current_max_tokens.min(self.efficiency_config.turn_token_budget as u32);
            }

            // DESIGN-1 FIX: Only check the cache on the first iteration and only
            // return cached if the cached value has no tool calls embedded.
            if iterations == 0 {
                if let Some(cached) = self.response_cache.get(&history_slice, &model_to_use) {
                    // Only use cached response if it is a plain-text final response
                    if !cached.contains("<tool_call") && !cached.contains("[TOOL_CALL]") {
                        return Ok(strip_tool_call_markup(&cached));
                    }
                }
            }

            // ── HERMES: Apply Anthropic prompt caching ────────────────
            // Reduces input token costs by ~75% on multi-turn conversations.
            let api_messages = if prompt_cache::supports_prompt_caching(&model_to_use, &active_provider.get_name()) {
                prompt_cache::apply_anthropic_cache_control(&history_slice, false)
            } else {
                history_slice.clone()
            };

            let request = ChatRequest {
                messages: &api_messages,
                model: &model_to_use,
                temperature: self.temperature,
                max_tokens: Some(current_max_tokens),
                tools: if tool_specs.is_empty() {
                    None
                } else {
                    Some(&tool_specs)
                },
                timeout_secs: 60,
                reasoning_effort: None,
            };

            // R-2: Retry logic for provider calls
            let mut last_error = None;
            let mut response = None;
            for attempt in 1..=3 {
                match active_provider.chat(&request) {
                    Ok(resp) => {
                        response = Some(resp);
                        break;
                    }
                    Err(e) => {
                        warn!("Provider chat attempt {} failed: {}", attempt, e);
                        last_error = Some(e);
                        if attempt < 3 {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                500 * attempt as u64,
                            ))
                            .await;
                        }
                    }
                }
            }

            let response = match response {
                Some(resp) => resp,
                None => {
                    let err = format!(
                        "❌ Error: I encountered a persistent issue communicating with the AI provider: {}. Please try again in a moment.",
                        last_error.unwrap()
                    );
                    if !accumulated_text.is_empty() {
                        return Ok(format!("{}\n\n{}", accumulated_text.trim(), err));
                    }
                    return Ok(err);
                }
            };

            // DESIGN-1 FIX: Only cache when there are no tool calls in the response.
            let has_native_tool_calls = !response.tool_calls.is_empty();
            let response_content_str = response.content.clone().unwrap_or_default();
            let has_xml_tool_calls = response_content_str.contains("<tool_call")
                || response_content_str.contains("[TOOL_CALL]");

            if !has_native_tool_calls && !has_xml_tool_calls {
                if let Some(ref content) = response.content {
                    self.response_cache
                        .insert(&self.history, &model_to_use, content.clone());
                }
            }

            // Extract any text present in this iteration (outside tool calls)
            // BUG-3 guard: only run strip if there is actual markup to strip
            let current_text = if has_xml_tool_calls {
                strip_tool_call_markup(&response_content_str)
            } else {
                response_content_str.trim().to_string()
            };
            if !is_low_value_response(&current_text) {
                if !accumulated_text.is_empty() {
                    accumulated_text.push_str("\n\n");
                }
                accumulated_text.push_str(&current_text);
            }

            // B-6: Record cost usage with real model-specific prices
            if let Some(tracker) = &mut self.cost_tracker {
                let (input_price, output_price) = crate::token_estimator::get_model_prices(&model_to_use);
                let token_usage = crate::cost::TokenUsage::new(
                    &model_to_use,
                    response.usage.prompt_tokens as u64,
                    response.usage.completion_tokens as u64,
                    input_price,
                    output_price,
                );
                let _ = tracker.record_usage(token_usage);
            }

            // ── HERMES: Update compressor with real token counts ──────
            // Only use prompt_tokens — completion/reasoning tokens don't consume
            // context window space. Thinking models inflate completion_tokens
            // with reasoning, causing premature compression.
            if let Some(ref mut compressor) = self.context_compressor {
                compressor.last_prompt_tokens = response.usage.prompt_tokens as u64;
                compressor.last_completion_tokens = response.usage.completion_tokens as u64;
            }

            // Append assistant response to live history
            let assistant_msg = ChatMessage {
                role: "assistant".to_string(),
                content: response.content.clone().unwrap_or_default(),
                name: None,
                tool_calls: if response.tool_calls.is_empty() {
                    None
                } else {
                    Some(response.tool_calls.clone())
                },
                thought_signature: response.thought_signature.clone(),
                tool_call_id: None,
                content_parts: None,
            };
            self.history.push(assistant_msg);

            // R-4: Handle tool calls (prioritize native, then XML)
            let tool_calls_to_execute: Vec<ParsedToolCall> = if has_native_tool_calls {
                // Native tool format (OpenAI/Anthropic function-calling)
                response
                    .tool_calls
                    .iter()
                    .map(|tc| ParsedToolCall {
                        name: tc.function.name.clone(),
                        arguments_json: tc.function.arguments.clone(),
                        tool_call_id: Some(tc.id.clone()),
                        thought_signature: tc.function.thought_signature.clone(),
                    })
                    .collect()
            } else {
                // Parse XML tool calls from content
                let parse_result = parse_tool_calls(&response_content_str);
                if parse_result.calls.is_empty() {
                    let display_text = response_content_str.clone();

                    // QW-6: Conditional follow-through guardrail
                    // BUG-2 FIX: Cap nudge retries at 2 to prevent infinite loops.
                    if follow_through_nudge_count < 2
                        && iterations < self.max_tool_iterations.saturating_sub(1)
                        && should_force_follow_through(&display_text, &self.tools)
                    {
                        // Extra guard: don't nudge if the model just repeated itself verbatim
                        let text_repeated = self.history.len() >= 3 && {
                            let prev_assistant = &self.history[self.history.len() - 3];
                            prev_assistant.role == "assistant"
                                && prev_assistant.content == display_text
                        };

                        if !text_repeated {
                            self.history.push(ChatMessage {
                                role: "user".to_string(),
                                content: "SYSTEM: You promised to take action now (e.g. \"I'll check now\" or \"Let me try\"). Do it in this turn by issuing the appropriate tool call(s). If no tool can perform it, explain the limitation clearly and do not promise a future attempt.".to_string(),
                                name: None,
                                tool_calls: None,
                                tool_call_id: None,
                                content_parts: None,
                    thought_signature: None,
                            });
                            follow_through_nudge_count += 1;
                            iterations += 1;
                            continue;
                        }
                    }

                        // No tool calls — genuine (or forced) final response
                    if accumulated_text.is_empty() {
                        let final_text = strip_tool_call_markup(&display_text);
                        if is_low_value_response(&final_text) {
                            let has_tool_results = self.history.iter().any(|m| m.role == "tool");

                            // BUG-2 FIX: Cap empty-response nudge at 1 attempt
                            if empty_response_nudge_count < 1 {
                                let nudge_content = if has_tool_results {
                                    "SYSTEM: You have finished calling tools. Now, provide a concise final response to the user summarizing the results or answering their question based on the tool outputs above. Do NOT use placeholder text like \"Done.\".".to_string()
                                } else {
                                    "SYSTEM: Your response was empty or too brief. Please provide a clear and concise answer or explanation to the user.".to_string()
                                };

                                self.history.push(ChatMessage {
                                    role: "user".to_string(),
                                    content: nudge_content,
                                    name: None,
                                    tool_calls: None,
                                    tool_call_id: None,
                                    content_parts: None,
                                    thought_signature: None,
                                });
                                empty_response_nudge_count += 1;
                                iterations += 1;
                                continue;
                            } else {
                                // Nudge was already sent once; return a meaningful fallback
                                if has_tool_results {
                                    self.spawn_background_review_if_needed();
                                    return Ok("I have completed the requested actions. [Detailed summary unavailable]".to_string());
                                }
                            }
                        }
                        self.spawn_background_review_if_needed();
                        return Ok(final_text);
                    }
                    self.spawn_background_review_if_needed();
                    return Ok(accumulated_text);
                }
                parse_result.calls
            };

            // Tool call deduplication: prevent the same (name, args) from being dispatched
            // twice in the same iteration (e.g. if the model emits duplicate blocks).
            let mut seen_calls = std::collections::HashSet::new();
            let deduped_calls: Vec<&ParsedToolCall> = tool_calls_to_execute
                .iter()
                .filter(|tc| {
                    let key = format!("{}:{}", tc.name, tc.arguments_json);
                    seen_calls.insert(key)
                })
                .collect();

            // BUG-7 FIX: Cross-iteration circuit breaker — if the same tool+args
            // has been called MAX_SAME_TOOL_RETRIES times, stop retrying and tell
            // the LLM to give up. Prevents infinite loops with failing tools.
            let mut blocked_tools: Vec<String> = Vec::new();
            for tc in &deduped_calls {
                let key = format!("{}:{}", tc.name, tc.arguments_json);
                let count = tool_call_counts.entry(key).or_insert(0);
                *count += 1;
                if *count > MAX_SAME_TOOL_RETRIES {
                    blocked_tools.push(tc.name.clone());
                }
            }

            if !blocked_tools.is_empty() {
                let blocked_names: Vec<String> = blocked_tools.into_iter().collect::<std::collections::HashSet<_>>().into_iter().collect();
                warn!(
                    "Circuit breaker: tools {:?} called {} times with same args, stopping retry loop",
                    blocked_names, MAX_SAME_TOOL_RETRIES
                );
                let circuit_msg = format!(
                    "SYSTEM: The following tool(s) have been called {} times with the same arguments and continue to fail: {}. \
                     Do NOT retry them. Instead, explain to the user what went wrong and suggest alternatives \
                     (e.g. a different approach, manual steps, or a different tool).",
                    MAX_SAME_TOOL_RETRIES,
                    blocked_names.join(", ")
                );
                self.history.push(ChatMessage {
                    role: "user".to_string(),
                    content: circuit_msg,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    content_parts: None,
                    thought_signature: None,
                });
                iterations += 1;
                continue;
            }

            let mut handles = Vec::new();
            for tool_call in &deduped_calls {
                let tool = self
                    .tools
                    .iter()
                    .find(|t| t.name() == tool_call.name)
                    .cloned();
                let call = (*tool_call).clone();
                let cache = self.tool_cache.clone();
                let ctx_clone = context.clone();
                let handle = tokio::spawn(async move {
                    if let Some(tool) = tool {
                        if tool.cacheable()
                            && let Some(cached_result) = cache.get(&call.name, &call.arguments_json)
                        {
                            let mut res = cached_result;
                            res.tool_call_id = call.tool_call_id.clone();
                            return res;
                        }

                        match serde_json::from_str::<serde_json::Value>(&call.arguments_json) {
                            Ok(args) => match tool.execute(args, &ctx_clone).await {
                                Ok(output) => {
                                    let res = ToolExecutionResult {
                                        name: call.name.clone(),
                                        output: output.content,
                                        success: true,
                                        tool_call_id: call.tool_call_id.clone(),
                                        thought_signature: call.thought_signature.clone(),
                                    };
                                    if tool.cacheable() {
                                        cache.insert(&call.name, &call.arguments_json, res.clone());
                                    }
                                    res
                                }
                                Err(e) => ToolExecutionResult {
                                    name: call.name.clone(),
                                    output: format!("Error executing tool: {}", e),
                                    success: false,
                                    tool_call_id: call.tool_call_id.clone(),
                                    thought_signature: call.thought_signature.clone(),
                                },
                            },
                            Err(e) => ToolExecutionResult {
                                name: call.name.clone(),
                                output: format!("Error parsing arguments: {}", e),
                                success: false,
                                tool_call_id: call.tool_call_id.clone(),
                                thought_signature: call.thought_signature.clone(),
                            },
                        }
                    } else {
                        ToolExecutionResult {
                            name: call.name.clone(),
                            output: format!("Tool not found: {}", call.name),
                            success: false,
                            tool_call_id: call.tool_call_id.clone(),
                            thought_signature: call.thought_signature.clone(),
                        }
                    }
                });
                handles.push(handle);
            }

            let mut execution_results = Vec::new();
            for handle in handles {
                match handle.await {
                    Ok(result) => execution_results.push(result),
                    Err(e) => tracing::error!("Tool execution task failed: {}", e),
                }
            }

            // Self-Learning: Increment skill iteration counter per tool-calling iteration
            if self.skill_nudge_interval > 0 {
                self.iters_since_skill += 1;
            }

            // Self-Learning: Reset nudge counters when relevant tools are used
            for result in &execution_results {
                match result.name.as_str() {
                    "memory_md" | "memory_store" => self.turns_since_memory = 0,
                    "skill_manage" => self.iters_since_skill = 0,
                    _ => {}
                }
            }

            // ── EFFICIENCY: Record skill executions for self-improvement ──
            if self.efficiency_config.skill_self_improvement {
                if let Some(ref journal) = self.skill_journal {
                    for result in &execution_results {
                        // Check if this tool belongs to a skill
                        let skill_name = self.tools.iter()
                            .find(|t| t.name() == result.name)
                            .map(|t| t.skill_name())
                            .unwrap_or(&result.name)
                            .to_string();
                        
                        if result.success {
                            journal.record_success(&skill_name, &result.name, &result.output);
                        } else {
                            journal.record_failure(&skill_name, &result.name, &result.output);
                        }
                    }
                }
            }

            // Append tool results to live history.
            // DESIGN-4 FIX: No "reflection prompt" injection — modern LLMs natively
            // understand tool result turns and will continue reasoning without it.
            // The old injection wasted tokens and caused premature "final answer" responses.

            // For non-native tool providers (Gemini, etc.), format as XML to avoid
            // strict API requirements for functionResponse turn ordering.
            // See: https://ai.google.dev/api/generate-content#functionresponse
            let use_native_tool_results = active_provider.supports_native_tools();

            if use_native_tool_results {
                // Native tool results: separate message per tool (OpenAI/Anthropic style)
                for result in &execution_results {
                    // Use clean content (no emoji prefix) for strict-schema providers
                    // (Anthropic/Gemini reject tool messages with unexpected formatting).
                    let formatted_output = if result.success {
                        result.output.clone()
                    } else {
                        format!("Error: {}", result.output)
                    };

                    self.history.push(ChatMessage {
                        role: "tool".to_string(),
                        content: formatted_output,
                        name: Some(result.name.clone()),
                        tool_calls: None,
                        tool_call_id: result.tool_call_id.clone(),
                        content_parts: None,
                        thought_signature: result.thought_signature.clone(),
                    });
                }
            } else {
                // XML-style tool results (Gemini, Ollama, etc.) - single user message
                // This avoids Gemini's strict requirement that functionResponse must
                // come IMMEDIATELY after functionCall with no intervening messages.
                let mut combined_output = String::from("[Tool results]\n");
                for result in &execution_results {
                    let status = if result.success { "ok" } else { "error" };
                    let output_text = if result.success {
                        result.output.as_str()
                    } else {
                        // Create binding to avoid temporary lifetime issue in Rust 1.92+
                        let error_output = format!("Error: {}", result.output);
                        combined_output.push_str(&format!(
                            "<tool_result name=\"{}\" status=\"{}\">\n{}\n</tool_result>\n",
                            result.name, status, error_output
                        ));
                        continue;
                    };
                    combined_output.push_str(&format!(
                        "<tool_result name=\"{}\" status=\"{}\">\n{}\n</tool_result>\n",
                        result.name, status, output_text
                    ));
                }

                self.history.push(ChatMessage {
                    role: "user".to_string(),
                    content: combined_output,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    content_parts: None,
                    thought_signature: None,
                });
            }

            // ── HERMES: Post-tool-turn context compression ────────────
            // Use real token counts from the API response to decide compression.
            // prompt_tokens + completion_tokens is the actual context size the
            // provider reported plus the assistant turn — a tight lower bound
            // for the next prompt.
            if let Some(ref mut compressor) = self.context_compressor {
                let real_tokens = if compressor.last_prompt_tokens > 0 {
                    compressor.last_prompt_tokens
                } else {
                    crate::token_estimator::estimate_history_tokens_rough(&self.history)
                };
                if compressor.should_compress(real_tokens) {
                    tracing::info!("  compacting context…");
                    let compaction_provider = self.cheap_provider.clone()
                        .unwrap_or_else(|| self.provider.clone());
                    self.history = compressor.compress(
                        &mut self.history,
                        Some(real_tokens),
                        None,
                        &compaction_provider,
                        &self.model_routing_config.cheap_model,
                    );
                }
            }

            // BUG-1 FIX: self.history is always current; next iteration clones fresh.
            iterations += 1;
        }
    }

    /// Summarizes the work done when max iterations are reached, instead of failing silently.
    async fn iterations_exhausted_summary(
        &mut self,
        history_slice: &[ChatMessage],
        model: &str,
    ) -> Result<String> {
        warn!("Tool iterations exhausted, requesting summary");

        let active_provider = if model == self.model_routing_config.cheap_model {
            self.cheap_provider.clone().unwrap_or_else(|| self.provider.clone())
        } else {
            self.provider.clone()
        };

        let mut summary_history = history_slice.to_vec();
        summary_history.push(ChatMessage {
            role: "user".to_string(),
            content: "SYSTEM: You have reached the maximum number of tool iterations. DO NOT call any more tools. Summarize what you have accomplished so far and what remains to be done.".to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
            thought_signature: None,
        });

        let request = ChatRequest {
            messages: &summary_history,
            model,
            temperature: 0.0, // Stable summary
            max_tokens: Some(1000),
            tools: None, // Force text-only
            timeout_secs: 30,
            reasoning_effort: None,
        };

        match active_provider.chat(&request) {
            Ok(resp) => {
                let content = resp.content.unwrap_or_else(|| {
                    "I reached my tool limit but couldn't generate a summary.".to_string()
                });
                Ok(format!(
                    "[Limit reached]\n\n{}",
                    strip_tool_call_markup(&content)
                ))
            }
            Err(e) => Ok(format!(
                "⚠️ I reached the maximum tool iteration limit and encountered an error while summarizing: {}. Please try again with a fresh session.",
                e
            )),
        }
    }

    /// Self-Learning: Spawn a background review if nudge counters have fired.
    fn spawn_background_review_if_needed(&mut self) {
        let should_review_memory = self.memory_nudge_interval > 0
            && self.turns_since_memory >= self.memory_nudge_interval;
        let should_review_skills = self.skill_nudge_interval > 0
            && self.iters_since_skill >= self.skill_nudge_interval;

        if should_review_memory {
            self.turns_since_memory = 0;
        }
        if should_review_skills {
            self.iters_since_skill = 0;
        }

        if !should_review_memory && !should_review_skills {
            return;
        }

        let provider = self.provider.clone();
        let cheap_provider = self.cheap_provider.clone();
        let model_name = self.model_name.clone();
        let tools = self.tools.clone();
        let workspace_dir = self.workspace_dir.clone();
        let history_snapshot = self.history.clone();

        background_review::spawn_background_review(
            provider,
            cheap_provider,
            model_name,
            tools,
            workspace_dir,
            history_snapshot,
            should_review_memory,
            should_review_skills,
        );
    }

    /// Self-Learning: Flush memories before session exit or reset.
    /// Appends a sentinel, runs a single-turn memory consolidation, then
    /// strips the sentinel and all flush artifacts (Hermes-style).
    pub async fn flush_memories(&mut self) {
        if self.memory_flush_min_turns == 0 {
            return;
        }
        if self.user_turn_count < self.memory_flush_min_turns as u64 {
            return;
        }

        let provider = self.provider.clone();
        let model_name = self.model_name.clone();
        let tools = self.tools.clone();

        background_review::flush_memories(
            provider,
            model_name,
            tools,
            &mut self.history,
        )
        .await;
    }

    /// Streaming variant of `turn()`. Emits text deltas to `callback` as tokens arrive.
    /// Returns the final complete response text (same as `turn()`).
    pub async fn turn_stream(
        &mut self,
        user_message: String,
        context: &ToolContext,
        callback: crate::providers::StreamCallback,
    ) -> Result<String> {
        use crate::model_router;
        use crate::providers::StreamChunk;

        let shared_callback = std::sync::Arc::new(std::sync::Mutex::new(callback));

        // Handle slash commands (no streaming needed)
        if let Some(slash_response) = commands::CommandRegistry::handle_message(self, &user_message)
        {
            if let Ok(mut cb) = shared_callback.lock() {
                cb(StreamChunk::Delta(slash_response.clone()));
                cb(StreamChunk::Done(crate::providers::TokenUsage::default()));
            }
            return Ok(slash_response);
        }

        // Self-Learning: Track user turns and memory nudge
        self.user_turn_count += 1;
        if self.memory_nudge_interval > 0 {
            self.turns_since_memory += 1;
        }

        // QW-7: Cheap model routing
        let model_to_use =
            model_router::route_to_appropriate_model(&user_message, &self.model_routing_config)
                .to_string();

        // Select the appropriate provider for the routed model
        let active_provider = if model_to_use == self.model_routing_config.cheap_model {
            self.cheap_provider.clone().unwrap_or_else(|| self.provider.clone())
        } else {
            self.provider.clone()
        };

        // R-6 & QW-2: Memory enrichment (only if substantive)
        let enriched_msg = if user_message.len() > 30
            && !model_router::is_greeting(&user_message)
            && self.memory.as_any().downcast_ref::<NoopMemory>().is_none()
        {
            match memory_loader::enrich_message(
                self.memory.as_ref(),
                &user_message,
                self.memory_session_id.as_deref(),
            ) {
                Ok(msg) => msg,
                Err(e) => {
                    warn!("Memory enrichment failed: {}", e);
                    user_message.clone()
                }
            }
        } else {
            user_message.clone()
        };

        // Hermes-style: inject dialectic user model into the user message (not system prompt)
        let dialectic = crate::dialectic::load_dialectic_context(&self.workspace_dir);
        let enriched_msg = if !dialectic.is_empty() {
            format!("<memory-context>\n{}\n</memory-context>\n\n{}", dialectic, enriched_msg)
        } else {
            enriched_msg
        };

        // R-1 & QW-2: Auto-save user message (only if substantive)
        // Skip saving imperative/task messages to prevent agent confusion
        if self.auto_save && user_message.len() > 20 && !model_router::is_greeting(&user_message) {
            let lower = user_message.to_lowercase();
            // Don't save imperative commands (create, delete, run, etc.) - only save facts/preferences
            let is_imperative = lower.contains("create")
                || lower.contains("delete")
                || lower.contains("run")
                || lower.contains("execute")
                || lower.contains("send")
                || lower.contains("write")
                || lower.contains("build")
                || lower.contains("generate");

            if !is_imperative {
                let key = format!(
                    "autosave_user_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_nanos()
                );
                let _ = self
                    .memory
                    .store(&key, &user_message, self.memory_session_id.as_deref());
            }
        }

        // R-3: System Prompt Management (Cached)
        if !self.has_system_prompt || self.cached_system_prompt.get().is_none() {
            let mut system_prompt = self.build_system_prompt_cached()?;

            // Always ensure the time is fresh in the prompt
            if let Some(start_idx) = system_prompt.find("## Current Date & Time") {
                let mut fresh_time = String::new();
                prompt::append_date_time_section(&mut fresh_time);

                let end_idx = system_prompt[start_idx + 22..]
                    .find("##")
                    .map(|i| i + start_idx + 22)
                    .unwrap_or(system_prompt.len());

                system_prompt.replace_range(start_idx..end_idx, &fresh_time);
            }

            // Insert or replace system prompt at index 0
            if !self.history.is_empty() && self.history[0].role == "system" {
                let mut new_content = system_prompt;
                if let Some(pos) = self.history[0]
                    .content
                    .find("\n\n--- COMPACTED PAST CONTEXT ---\n")
                {
                    new_content.push_str(&self.history[0].content[pos..]);
                }
                self.history[0].content = new_content;
            } else {
                self.history.insert(
                    0,
                    ChatMessage {
                        role: "system".to_string(),
                        content: system_prompt,
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                        content_parts: None,
                        thought_signature: None,
                    },
                );
            }
            self.has_system_prompt = true;
        } else if !self.history.is_empty() && self.history[0].role == "system" {
            // Even if the system prompt was set manually or is already present,
            // try to refresh the time section if it exists.
            let mut content = self.history[0].content.clone();
            if let Some(start_idx) = content.find("## Current Date & Time") {
                let mut fresh_time = String::new();
                prompt::append_date_time_section(&mut fresh_time);

                let end_idx = content[start_idx + 22..]
                    .find("##")
                    .map(|i| i + start_idx + 22)
                    .unwrap_or(content.len());

                content.replace_range(start_idx..end_idx, &fresh_time);
                self.history[0].content = content;
            }
        }

        let (final_content, content_parts) = self.parse_multimodal_message(&enriched_msg);

        // Append user message
        self.history.push(ChatMessage {
            role: "user".to_string(),
            content: final_content,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts,
            thought_signature: None,
        });

        // Enforce history limits
        trim_history(&mut self.history, self.max_history_messages as u32);

        // Auto-compaction (legacy fallback)
        if self.token_limit > 0 {
            let compaction_config = crate::agent::compaction::LegacyCompactionConfig {
                keep_recent: 10,
                max_summary_chars: 2000,
                max_source_chars: 12000,
                token_limit: self.token_limit,
                max_history_messages: self.max_history_messages as u32,
                workspace_dir: Some(self.workspace_dir.clone()),
            };
            let compaction_provider = self.cheap_provider.as_ref().unwrap_or(&self.provider);
            let _ = auto_compact_history_legacy(
                &mut self.history,
                compaction_provider,
                &self.model_routing_config.cheap_model,
                &compaction_config,
                &mut self.last_compaction,
            );
        }

        // Prepare ToolSpecs once (outside the loop)
        let tool_specs: Vec<ToolSpec> = self
            .tools
            .iter()
            .map(|t| {
                let params: serde_json::Value =
                    serde_json::from_str(&t.parameters_json()).unwrap_or(serde_json::json!({}));
                ToolSpec {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: params,
                }
            })
            .collect();

        // BUG-3 FIX: Removed short-message history slicing heuristic.
        // BUG-1 FIX: history_slice is rebuilt fresh INSIDE the loop each iteration.

        // Tool loop with streaming
        let mut iterations = 0u32;
        // BUG-2 FIX: Independent counters per nudge type to prevent infinite loops.
        let mut follow_through_nudge_count = 0u32;
        let mut empty_response_nudge_count = 0u32;
        // BUG-7 FIX: Cross-iteration circuit breaker (same as turn()).
        let mut tool_call_counts: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();

        loop {
            if iterations >= self.max_tool_iterations {
                warn!("Max tool iterations reached");
                // BUG-4 FIX: Clone live history to satisfy borrow checker
                // (&mut self cannot coexist with &self.history in a method call).
                let history_for_summary = self.history.clone();
                return self
                    .iterations_exhausted_summary(&history_for_summary, &model_to_use)
                    .await;
            }

            // BUG-1 FIX: Rebuild fresh every iteration so the model always
            // sees the latest tool results and assistant messages.
            // CONTEXT SLICING: On the first iteration, slice trivial follow-ups
            // to save tokens; subsequent iterations need full history for tool results.
            let history_slice = if iterations == 0 {
                select_context_window(&self.history, &user_message)
            } else {
                self.history.clone()
            };

            // Resolve adaptive max tokens with fresh history length
            let mut current_max_tokens =
                max_tokens_resolver::resolve_max_tokens(Some(history_slice.len()), &model_to_use);

            // Enforce per-turn token budget from config
            if self.efficiency_config.turn_token_budget > 0 {
                current_max_tokens = current_max_tokens.min(self.efficiency_config.turn_token_budget as u32);
            }

            // DESIGN-1 FIX: Only check cache on first iteration, and skip if cached
            // response contains tool calls (which would be garbled if returned raw).
            if iterations == 0 {
                if let Some(cached) = self.response_cache.get(&history_slice, &model_to_use) {
                    if !cached.contains("<tool_call") && !cached.contains("[TOOL_CALL]") {
                        return Ok(strip_tool_call_markup(&cached));
                    }
                }
            }

            let request = ChatRequest {
                messages: &history_slice,
                model: &model_to_use,
                temperature: self.temperature,
                max_tokens: Some(current_max_tokens),
                tools: if tool_specs.is_empty() {
                    None
                } else {
                    Some(&tool_specs)
                },
                timeout_secs: 60,
                reasoning_effort: None,
            };

            // Use streaming: collect full text via shared accumulator
            let accumulated = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
            // R-2: Retry logic for streaming provider calls
            let mut last_error = None;
            let mut response = None;
            for attempt in 1..=3 {
                let acc_retry = accumulated.clone();
                let cb_retry = shared_callback.clone();
                let stream_cb: crate::providers::StreamCallback =
                    Box::new(move |chunk: StreamChunk| {
                        if let StreamChunk::Delta(ref text) = chunk
                            && let Ok(mut acc) = acc_retry.lock()
                        {
                            acc.push_str(text);
                        }
                        if let Ok(mut cb) = cb_retry.lock() {
                            cb(chunk);
                        }
                    });

                match active_provider.chat_stream(&request, stream_cb) {
                    Ok(resp) => {
                        response = Some(resp);
                        break;
                    }
                    Err(e) => {
                        warn!("Provider chat_stream attempt {} failed: {}", attempt, e);
                        last_error = Some(e);
                        if attempt < 3 {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                500 * attempt as u64,
                            ))
                            .await;
                        }
                    }
                }
            }

            let response = match response {
                Some(resp) => resp,
                None => {
                    let err_msg = format!(
                        "❌ Error: I encountered a persistent issue communicating with the AI provider: {}. Please try again in a moment.",
                        last_error.unwrap()
                    );
                    if let Ok(mut cb) = shared_callback.lock() {
                        cb(StreamChunk::Delta(err_msg.clone()));
                        cb(StreamChunk::Done(crate::providers::TokenUsage::default()));
                    }
                    return Ok(err_msg);
                }
            };

            // DESIGN-1 FIX: Only cache final text responses, not tool-call responses.
            let has_native_tool_calls_s = !response.tool_calls.is_empty();
            let response_content_str_s = response.content.clone().unwrap_or_default();
            let has_xml_tool_calls_s = response_content_str_s.contains("<tool_call")
                || response_content_str_s.contains("[TOOL_CALL]");

            if !has_native_tool_calls_s && !has_xml_tool_calls_s {
                if let Some(ref content) = response.content {
                    self.response_cache
                        .insert(&self.history, &model_to_use, content.clone());
                }
            }

            // B-6: Record cost usage with real model-specific prices
            if let Some(tracker) = &mut self.cost_tracker {
                let (input_price, output_price) = crate::token_estimator::get_model_prices(&model_to_use);
                let token_usage = crate::cost::TokenUsage::new(
                    &model_to_use,
                    response.usage.prompt_tokens as u64,
                    response.usage.completion_tokens as u64,
                    input_price,
                    output_price,
                );
                let _ = tracker.record_usage(token_usage);
            }

            // Append assistant response
            let assistant_msg = ChatMessage {
                role: "assistant".to_string(),
                content: response.content.clone().unwrap_or_default(),
                name: None,
                tool_calls: if response.tool_calls.is_empty() {
                    None
                } else {
                    Some(response.tool_calls.clone())
                },
                thought_signature: response.thought_signature.clone(),
                tool_call_id: None,
                content_parts: None,
            };
            self.history.push(assistant_msg);

            // Handle tool calls (prioritize native, then XML)
            let tool_calls_to_execute: Vec<ParsedToolCall> = if has_native_tool_calls_s {
                response
                    .tool_calls
                    .iter()
                    .map(|tc| ParsedToolCall {
                        name: tc.function.name.clone(),
                        arguments_json: tc.function.arguments.clone(),
                        tool_call_id: Some(tc.id.clone()),
                        thought_signature: tc.function.thought_signature.clone(),
                    })
                    .collect()
            } else {
                let parse_result = parse_tool_calls(&response_content_str_s);
                if parse_result.calls.is_empty() {
                    let display_text = response_content_str_s.clone();

                    // BUG-2 FIX: Cap nudge retries at 2
                    if follow_through_nudge_count < 2
                        && iterations < self.max_tool_iterations.saturating_sub(1)
                        && should_force_follow_through(&display_text, &self.tools)
                    {
                        let text_repeated = self.history.len() >= 3 && {
                            let prev_assistant = &self.history[self.history.len() - 3];
                            prev_assistant.role == "assistant"
                                && prev_assistant.content == display_text
                        };
                        if !text_repeated {
                            self.history.push(ChatMessage {
                                role: "user".to_string(),
                                content: "SYSTEM: You promised to take action now (e.g. \"I'll check now\" or \"Let me try\"). Do it in this turn by issuing the appropriate tool call(s). If no tool can perform it, explain the limitation clearly and do not promise a future attempt.".to_string(),
                                name: None,
                                tool_calls: None,
                                tool_call_id: None,
                                content_parts: None,
                    thought_signature: None,
                            });
                            follow_through_nudge_count += 1;
                            iterations += 1;
                            continue;
                        }
                    }

                    let final_text = strip_tool_call_markup(&display_text);
                    if is_low_value_response(&final_text) {
                        let has_tool_results = self.history.iter().any(|m| m.role == "tool");
                        if empty_response_nudge_count < 1 {
                            let nudge_content = if has_tool_results {
                                "SYSTEM: You have finished calling tools. Now, provide a concise final response to the user summarizing the results or answering their question based on the tool outputs above. Do NOT use placeholder text like \"Done.\"." .to_string()
                            } else {
                                "SYSTEM: Your response was empty or too brief. Please provide a clear and concise answer or explanation to the user.".to_string()
                            };
                            self.history.push(ChatMessage {
                                role: "user".to_string(),
                                content: nudge_content,
                                name: None,
                                tool_calls: None,
                                tool_call_id: None,
                                content_parts: None,
                                thought_signature: None,
                            });
                            empty_response_nudge_count += 1;
                            iterations += 1;
                            continue;
                        } else {
                            if has_tool_results {
                                self.spawn_background_review_if_needed();
                                return Ok("I have completed the requested actions. [Detailed summary unavailable]".to_string());
                            }
                        }
                    }
                    self.spawn_background_review_if_needed();
                    return Ok(final_text);
                }
                parse_result.calls
            };

            // Tool call deduplication for turn_stream
            let mut seen_calls_s = std::collections::HashSet::new();
            let deduped_calls_s: Vec<&ParsedToolCall> = tool_calls_to_execute
                .iter()
                .filter(|tc| {
                    let key = format!("{}:{}", tc.name, tc.arguments_json);
                    seen_calls_s.insert(key)
                })
                .collect();

            // BUG-7 FIX: Cross-iteration circuit breaker (same as turn()).
            let mut blocked_tools_s: Vec<String> = Vec::new();
            for tc in &deduped_calls_s {
                let key = format!("{}:{}", tc.name, tc.arguments_json);
                let count = tool_call_counts.entry(key).or_insert(0);
                *count += 1;
                if *count > MAX_SAME_TOOL_RETRIES {
                    blocked_tools_s.push(tc.name.clone());
                }
            }

            if !blocked_tools_s.is_empty() {
                let blocked_names_s: Vec<String> = blocked_tools_s.into_iter().collect::<std::collections::HashSet<_>>().into_iter().collect();
                warn!(
                    "Circuit breaker: tools {:?} called {} times with same args, stopping retry loop",
                    blocked_names_s, MAX_SAME_TOOL_RETRIES
                );
                let circuit_msg = format!(
                    "SYSTEM: The following tool(s) have been called {} times with the same arguments and continue to fail: {}. \
                     Do NOT retry them. Instead, explain to the user what went wrong and suggest alternatives \
                     (e.g. a different approach, manual steps, or a different tool).",
                    MAX_SAME_TOOL_RETRIES,
                    blocked_names_s.join(", ")
                );
                self.history.push(ChatMessage {
                    role: "user".to_string(),
                    content: circuit_msg,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    content_parts: None,
                    thought_signature: None,
                });
                iterations += 1;
                continue;
            }

            let mut handles = Vec::new();
            for tool_call in &deduped_calls_s {
                let tool = self
                    .tools
                    .iter()
                    .find(|t| t.name() == tool_call.name)
                    .cloned();
                let call = (*tool_call).clone();
                let cache = self.tool_cache.clone();
                let ctx_clone = context.clone();
                let handle = tokio::spawn(async move {
                    if let Some(tool) = tool {
                        if tool.cacheable()
                            && let Some(cached_result) = cache.get(&call.name, &call.arguments_json)
                        {
                            let mut res = cached_result;
                            res.tool_call_id = call.tool_call_id.clone();
                            return res;
                        }

                        match serde_json::from_str::<serde_json::Value>(&call.arguments_json) {
                            Ok(args) => match tool.execute(args, &ctx_clone).await {
                                Ok(output) => {
                                    let res = ToolExecutionResult {
                                        name: call.name.clone(),
                                        output: output.content,
                                        success: true,
                                        tool_call_id: call.tool_call_id.clone(),
                                        thought_signature: call.thought_signature.clone(),
                                    };
                                    if tool.cacheable() {
                                        cache.insert(&call.name, &call.arguments_json, res.clone());
                                    }
                                    res
                                }
                                Err(e) => ToolExecutionResult {
                                    name: call.name.clone(),
                                    output: format!("Error executing tool: {}", e),
                                    success: false,
                                    tool_call_id: call.tool_call_id.clone(),
                                    thought_signature: call.thought_signature.clone(),
                                },
                            },
                            Err(e) => ToolExecutionResult {
                                name: call.name.clone(),
                                output: format!("Error parsing arguments: {}", e),
                                success: false,
                                tool_call_id: call.tool_call_id.clone(),
                                thought_signature: call.thought_signature.clone(),
                            },
                        }
                    } else {
                        ToolExecutionResult {
                            name: call.name.clone(),
                            output: format!("Tool not found: {}", call.name),
                            success: false,
                            tool_call_id: call.tool_call_id.clone(),
                            thought_signature: call.thought_signature.clone(),
                        }
                    }
                });
                handles.push(handle);
            }

            let mut execution_results = Vec::new();
            for handle in handles {
                match handle.await {
                    Ok(result) => execution_results.push(result),
                    Err(e) => tracing::error!("Tool execution task failed: {}", e),
                }
            }

            // Self-Learning: Increment skill iteration counter per tool-calling iteration
            if self.skill_nudge_interval > 0 {
                self.iters_since_skill += 1;
            }

            // Self-Learning: Reset nudge counters when relevant tools are used
            for result in &execution_results {
                match result.name.as_str() {
                    "memory_md" | "memory_store" => self.turns_since_memory = 0,
                    "skill_manage" => self.iters_since_skill = 0,
                    _ => {}
                }
            }

            // DESIGN-4 FIX: No reflection prompt; modern LLMs handle tool result turns natively.
            // BUG-1 FIX: self.history is always current; next iteration clones fresh.

            // For non-native tool providers (Gemini, etc.), format as XML to avoid
            // strict API requirements for functionResponse turn ordering.
            let use_native_tool_results_stream = active_provider.supports_native_tools();

            if use_native_tool_results_stream {
                // Native tool results: separate message per tool (OpenAI/Anthropic style)
                for result in &execution_results {
                    let formatted_output = if result.success {
                        result.output.clone()
                    } else {
                        format!("Error: {}", result.output)
                    };
                    self.history.push(ChatMessage {
                        role: "tool".to_string(),
                        content: formatted_output,
                        name: Some(result.name.clone()),
                        tool_calls: None,
                        tool_call_id: result.tool_call_id.clone(),
                        content_parts: None,
                        thought_signature: result.thought_signature.clone(),
                    });
                }
            } else {
                // XML-style tool results (Gemini, Ollama, etc.) - single user message
                let mut combined_output_s = String::from("[Tool results]\n");
                for result in &execution_results {
                    let status = if result.success { "ok" } else { "error" };
                    let output_text_s = if result.success {
                        result.output.as_str()
                    } else {
                        // Create binding to avoid temporary lifetime issue in Rust 1.92+
                        let error_output_s = format!("Error: {}", result.output);
                        combined_output_s.push_str(&format!(
                            "<tool_result name=\"{}\" status=\"{}\">\n{}\n</tool_result>\n",
                            result.name, status, error_output_s
                        ));
                        continue;
                    };
                    combined_output_s.push_str(&format!(
                        "<tool_result name=\"{}\" status=\"{}\">\n{}\n</tool_result>\n",
                        result.name, status, output_text_s
                    ));
                }

                self.history.push(ChatMessage {
                    role: "user".to_string(),
                    content: combined_output_s,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    content_parts: None,
                    thought_signature: None,
                });
            }

            iterations += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_tool_call_markup_xml() {
        let input = "Here is the result. <tool_call>{\"name\": \"foo\"}</tool_call> Thanks.";
        let out = strip_tool_call_markup(input);
        assert_eq!(out, "Here is the result.  Thanks.".trim());
    }

    #[test]
    fn test_strip_tool_call_markup_bracket() {
        let input = "Done [tool_call]{json}[/tool_call] end";
        let out = strip_tool_call_markup(input);
        assert_eq!(out, "Done  end".trim());
    }

    #[test]
    fn test_strip_tool_call_markup_fenced() {
        let input = "Some text\n```tool_call\n{\"fn\": \"bar\"}\n```\nMore text";
        let out = strip_tool_call_markup(input);
        assert!(out.contains("Some text"));
        assert!(out.contains("More text"));
        assert!(!out.contains("tool_call"));
    }

    #[test]
    fn test_strip_tool_call_markup_empty_fallback() {
        let input = "<tool_call>everything</tool_call>";
        let out = strip_tool_call_markup(input);
        assert_eq!(out, "");
    }

    #[test]
    fn test_strip_tool_call_markup_clean_text() {
        let input = "This is a normal response without any tool calls.";
        let out = strip_tool_call_markup(input);
        assert_eq!(out, input);
    }

    #[test]
    fn test_should_force_follow_through() {
        use std::sync::Arc;
        struct MockTool;
        #[async_trait::async_trait]
        impl crate::tools::Tool for MockTool {
            fn name(&self) -> &str {
                "test_tool"
            }
            fn description(&self) -> &str {
                "desc"
            }
            fn parameters_json(&self) -> String {
                "{}".into()
            }
            async fn execute(
                &self,
                _: serde_json::Value,
                _: &crate::tools::ToolContext,
            ) -> anyhow::Result<crate::tools::ToolResult> {
                Ok(crate::tools::ToolResult::ok("ok"))
            }
        }
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool)];

        assert!(should_force_follow_through("Let me try that now:", &tools));
        assert!(should_force_follow_through(
            "I will get it done now.",
            &tools
        ));
        assert!(should_force_follow_through("I'll check the logs", &tools));
        assert!(should_force_follow_through(
            "Let's see what I can do.",
            &tools
        ));
        assert!(should_force_follow_through(
            "Let me try a different approach:",
            &tools
        ));
        assert!(should_force_follow_through(
            "So I need to use action: 'execute' with the exact tool slug. Let me try that now:",
            &tools
        ));
        assert!(should_force_follow_through(
            "I should probably check the file again.",
            &tools
        ));
        assert!(!should_force_follow_through(
            "I have finished the task.",
            &tools
        ));
        assert!(!should_force_follow_through(
            "Let me know if you need anything else.",
            &tools
        ));
    }

    #[test]
    fn test_is_low_value_response() {
        assert!(is_low_value_response(""));
        assert!(is_low_value_response("   "));
        assert!(is_low_value_response("✅ Done."));
        assert!(is_low_value_response("Done."));
        assert!(is_low_value_response("ok."));
        assert!(is_low_value_response("!!!"));
        assert!(is_low_value_response("..."));
        assert!(is_low_value_response("  \n  "));

        assert!(!is_low_value_response(
            "I have completed the task and found that everything is in order."
        ));
        assert!(!is_low_value_response("The sum of 2 and 2 is 4."));
    }
}

