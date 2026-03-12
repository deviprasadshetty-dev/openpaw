pub mod commands;
pub mod compaction;
pub mod context_tokens;
pub mod dispatcher;
pub mod max_tokens;
pub mod memory_loader;
pub mod prompt;
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

use compaction::{CompactionConfig, auto_compact_history, trim_history};
use context_tokens as context_tokens_resolver;
use dispatcher::{ParsedToolCall, ToolExecutionResult, parse_tool_calls};
use max_tokens as max_tokens_resolver;
use memory_loader::{Memory, NoopMemory};

pub struct Agent {
    pub provider: Arc<dyn Provider>,
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
}

/// QW-6: Conditional follow-through detection.
/// Returns true if the LLM output contains phrases indicating it intends to take action,
/// but didn't actually emit a tool call.
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
        "try", "check", "run", "do", "get", "execute", "look", "apply", "fix", "start", "use",
        "search", "fetch", "read", "write", "list", "ls", "edit", "update", "modify", "patch",
        "delete", "remove", "create", "make", "find",
    ];
    // Immediacy indicators
    const IMMEDIACY: &[&str] = &["now", "again", "immediately", "straight away", "instead"];

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
    let ends_with_colon = trimmed.ends_with(':');

    // 1. High confidence trigger: Starter + Verb + (Immediacy OR Colon)
    if has_starter && has_verb && (has_immediacy || ends_with_colon) {
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

        if has_relevant_tool
            && let Some(last_line) = trimmed.lines().last() {
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
///     Returns a trimmed result; falls back to "✅ Done." if everything was stripped.
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
    if trimmed.is_empty() {
        "\u{2705} Done.".to_string()
    } else {
        trimmed
    }
}

impl Agent {
    pub fn new(
        provider: Arc<dyn Provider>,
        tools: Vec<Arc<dyn Tool>>,
        model_name: String,
        workspace_dir: String,
    ) -> Self {
        // Resolve token limits using new modules
        let token_limit = context_tokens_resolver::resolve_context_tokens(None, &model_name);
        let max_tokens = max_tokens_resolver::resolve_max_tokens(None, &model_name);

        let mut agent = Self {
            provider,
            tools,
            memory: Arc::new(NoopMemory), // Default to no-op
            model_name: model_name.clone(),
            temperature: 0.7,
            max_tokens,
            token_limit,
            max_tool_iterations: 25,
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
        };

        if context_tokens_resolver::is_small_model_context(token_limit) {
            agent.max_tool_iterations = 100; // Increased for local models
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

        if let Some(cached) = self.cached_system_prompt.get()
            && current_tool_hash == self.last_tool_hash
                && Some(workspace_fp) == self.workspace_prompt_fingerprint
            {
                return Ok(cached.clone());
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
        let conversation_context = None; // Not yet integrated

        let ctx = prompt::PromptContext {
            workspace_dir: &self.workspace_dir,
            model_name: &self.model_name,
            tools: &self.tools,
            capabilities_section: caps,
            conversation_context,
            use_native_tools: self.provider.supports_native_tools(),
            token_limit: self.token_limit,
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

        // QW-7: Cheap model routing for greetings/simple tasks
        let model_to_use =
            model_router::route_to_appropriate_model(&user_message, &self.model_routing_config)
                .to_string();

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
            let system_prompt = self.build_system_prompt_cached()?;

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
                    },
                );
            }
            self.has_system_prompt = true;
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

        // Append user message
        self.history.push(ChatMessage {
            role: "user".to_string(),
            content: enriched_msg.clone(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
        });

        // Enforce history limits
        trim_history(&mut self.history, self.max_history_messages as u32);

        // Attempt auto-compaction if enabled and over token limit
        if self.token_limit > 0 {
            let compaction_config = CompactionConfig {
                keep_recent: 10,
                max_summary_chars: 2000,
                max_source_chars: 12000,
                token_limit: self.token_limit,
                max_history_messages: self.max_history_messages as u32,
                workspace_dir: Some(self.workspace_dir.clone()),
            };

            let _ = auto_compact_history(
                &mut self.history,
                &self.provider,
                &self.model_name,
                &compaction_config,
            );
        }

        // QW-6: Conditional follow-through detection
        let current_max_tokens =
            max_tokens_resolver::resolve_max_tokens(Some(self.history.len()), &model_to_use);

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

        // B-3: Context window slicing
        let history_slice = if enriched_msg.len() < 100 && !enriched_msg.contains("actually") {
            // Short message, potentially a follow-up. Use last 10 + system.
            if self.history.len() > 11 {
                let mut slice = vec![self.history[0].clone()];
                slice.extend(self.history[self.history.len() - 10..].iter().cloned());
                slice
            } else {
                self.history.clone()
            }
        } else {
            self.history.clone()
        };

        // Loop for tools
        let mut iterations = 0;
        let mut accumulated_text = String::new();

        loop {
            if iterations >= self.max_tool_iterations {
                warn!("Max tool iterations reached");
                let summary = self
                    .iterations_exhausted_summary(&history_slice, &model_to_use)
                    .await?;
                if !accumulated_text.is_empty() {
                    return Ok(format!("{}\n\n{}", accumulated_text.trim(), summary));
                }
                return Ok(summary);
            }

            // M-2: Check LLM response cache
            if let Some(cached) = self.response_cache.get(&history_slice, &model_to_use) {
                return Ok(strip_tool_call_markup(&cached));
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

            // R-2: Retry logic for provider calls
            let mut last_error = None;
            let mut response = None;
            for attempt in 1..=3 {
                match self.provider.chat(&request) {
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

            // M-2: Store in cache
            if let Some(ref content) = response.content {
                self.response_cache
                    .insert(&self.history, &model_to_use, content.clone());
            }

            // Extract any text present in this iteration (outside tool calls)
            let current_text =
                strip_tool_call_markup(&response.content.clone().unwrap_or_default());
            if current_text != "✅ Done." && !current_text.is_empty() {
                if !accumulated_text.is_empty() {
                    accumulated_text.push_str("\n\n");
                }
                accumulated_text.push_str(&current_text);
            }

            // B-6: Record cost usage
            if let Some(tracker) = &mut self.cost_tracker {
                let token_usage = crate::cost::TokenUsage::new(
                    &model_to_use,
                    response.usage.prompt_tokens as u64,
                    response.usage.completion_tokens as u64,
                    0.01, // Fallback prices if not in config
                    0.03,
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
                tool_call_id: None,
                content_parts: None,
            };
            self.history.push(assistant_msg);

            // R-4: Handle tool calls (prioritize native, strip markup)
            let tool_calls_to_execute: Vec<ParsedToolCall> = if !response.tool_calls.is_empty() {
                // Native tool format
                response
                    .tool_calls
                    .iter()
                    .map(|tc| ParsedToolCall {
                        name: tc.function.name.clone(),
                        arguments_json: tc.function.arguments.clone(),
                        tool_call_id: Some(tc.id.clone()),
                    })
                    .collect()
            } else {
                // Parse XML tool calls from content
                let parse_result = parse_tool_calls(&response.content.clone().unwrap_or_default());
                if parse_result.calls.is_empty() {
                    let display_text = response.content.clone().unwrap_or_default();

                    // QW-6: Conditional follow-through guardrail
                    if iterations < self.max_tool_iterations.saturating_sub(1)
                        && should_force_follow_through(&display_text, &self.tools)
                    {
                        // BREAK REPETITION LOOP: Check if we just sent this exact nudge 
                        // OR if the model is just repeating the exact same text.
                        let is_repeating = if self.history.len() >= 2 {
                            let prev_msg = &self.history[self.history.len() - 2];
                            // If history[len-1] is the current response (pushed above), 
                            // history[len-2] is the previous SYSTEM nudge or user message.
                            // If we want to check the PREVIOUS assistant message, it's history[len-3].
                            let prev_assistant = if self.history.len() >= 3 {
                                Some(&self.history[self.history.len() - 3])
                            } else {
                                None
                            };

                            let nudge_already_sent = prev_msg.role == "user" && prev_msg.content.contains("SYSTEM: You promised to take action now");
                            let text_repeated = prev_assistant.map(|m| m.role == "assistant" && m.content == display_text).unwrap_or(false);
                            
                            nudge_already_sent || text_repeated
                        } else {
                            false
                        };

                        if !is_repeating {
                            self.history.push(ChatMessage {
                                role: "user".to_string(),
                                content: "SYSTEM: You promised to take action now (e.g. \"I'll check now\" or \"Let me try\"). Do it in this turn by issuing the appropriate tool call(s). If no tool can perform it, explain the limitation clearly and do not promise a future attempt.".to_string(),
                                name: None,
                                tool_calls: None,
                                tool_call_id: None,
                                content_parts: None,
                            });
                            iterations += 1;
                            continue;
                        }
                    }

                    // No tool calls, no promise — genuine final response
                    if accumulated_text.is_empty() {
                        return Ok(current_text);
                    }
                    return Ok(accumulated_text);
                }
                parse_result.calls
            };

            let mut handles = Vec::new();
            for tool_call in &tool_calls_to_execute {
                let tool = self
                    .tools
                    .iter()
                    .find(|t| t.name() == tool_call.name)
                    .cloned();
                let call = tool_call.clone();
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
                                },
                            },
                            Err(e) => ToolExecutionResult {
                                name: call.name.clone(),
                                output: format!("Error parsing arguments: {}", e),
                                success: false,
                                tool_call_id: call.tool_call_id.clone(),
                            },
                        }
                    } else {
                        ToolExecutionResult {
                            name: call.name.clone(),
                            output: format!("Tool not found: {}", call.name),
                            success: false,
                            tool_call_id: call.tool_call_id.clone(),
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

            // Append tool results to history with reflection prompt
            for result in execution_results {
                let formatted_output = if result.success {
                    result.output
                } else {
                    format!("❌ Tool Error ({}): {}", result.name, result.output)
                };

                self.history.push(ChatMessage {
                    role: "tool".to_string(),
                    content: formatted_output,
                    name: Some(result.name),
                    tool_calls: None,
                    tool_call_id: result.tool_call_id,
                    content_parts: None,
                });
            }

            // Add reflection prompt for the next iteration
            self.history.push(ChatMessage {
                role: "user".to_string(),
                content: "Reflect on the tool results above and decide your next steps. If a tool failed, try a different approach or fix the parameters. If you have enough info, provide a final answer.".to_string(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            });

            // ── CRITICAL: re-enter loop so the provider sees tool results ──
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

        let mut summary_history = history_slice.to_vec();
        summary_history.push(ChatMessage {
            role: "user".to_string(),
            content: "SYSTEM: You have reached the maximum number of tool iterations. DO NOT call any more tools. Summarize what you have accomplished so far and what remains to be done.".to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
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

        match self.provider.chat(&request) {
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

        // QW-7: Cheap model routing
        let model_to_use =
            model_router::route_to_appropriate_model(&user_message, &self.model_routing_config)
                .to_string();

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

        // R-1 & QW-2: Auto-save user message (only if substantive)
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
            let system_prompt = self.build_system_prompt_cached()?;

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
                    },
                );
            }
            self.has_system_prompt = true;
        }

        // Append user message
        self.history.push(ChatMessage {
            role: "user".to_string(),
            content: enriched_msg.clone(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
        });

        // Enforce history limits
        trim_history(&mut self.history, self.max_history_messages as u32);

        // Auto-compaction
        if self.token_limit > 0 {
            let compaction_config = CompactionConfig {
                keep_recent: 10,
                max_summary_chars: 2000,
                max_source_chars: 12000,
                token_limit: self.token_limit,
                max_history_messages: self.max_history_messages as u32,
                workspace_dir: Some(self.workspace_dir.clone()),
            };
            let _ = auto_compact_history(
                &mut self.history,
                &self.provider,
                &self.model_name,
                &compaction_config,
            );
        }

        // B-1: Resolve adaptive max tokens
        let current_max_tokens =
            max_tokens_resolver::resolve_max_tokens(Some(self.history.len()), &model_to_use);

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

        // B-3: Context window slicing
        let history_slice = if enriched_msg.len() < 100 && !enriched_msg.contains("actually") {
            if self.history.len() > 11 {
                let mut slice = vec![self.history[0].clone()];
                slice.extend(self.history[self.history.len() - 10..].iter().cloned());
                slice
            } else {
                self.history.clone()
            }
        } else {
            self.history.clone()
        };

        // Tool loop with streaming
        let mut iterations = 0;
        loop {
            if iterations >= self.max_tool_iterations {
                warn!("Max tool iterations reached");
                return self
                    .iterations_exhausted_summary(&history_slice, &model_to_use)
                    .await;
            }

            // M-2: Check LLM response cache
            if let Some(cached) = self.response_cache.get(&history_slice, &model_to_use) {
                return Ok(strip_tool_call_markup(&cached));
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
                            && let Ok(mut acc) = acc_retry.lock() {
                                acc.push_str(text);
                            }
                        if let Ok(mut cb) = cb_retry.lock() {
                            cb(chunk);
                        }
                    });

                match self.provider.chat_stream(&request, stream_cb) {
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

            // M-2: Store in cache
            if let Some(ref content) = response.content {
                self.response_cache
                    .insert(&self.history, &model_to_use, content.clone());
            }

            // B-6: Record cost usage
            if let Some(tracker) = &mut self.cost_tracker {
                let token_usage = crate::cost::TokenUsage::new(
                    &model_to_use,
                    response.usage.prompt_tokens as u64,
                    response.usage.completion_tokens as u64,
                    0.01,
                    0.03,
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
                tool_call_id: None,
                content_parts: None,
            };
            self.history.push(assistant_msg);

            // Handle tool calls (same as turn())
            let tool_calls_to_execute: Vec<ParsedToolCall> = if !response.tool_calls.is_empty() {
                response
                    .tool_calls
                    .iter()
                    .map(|tc| ParsedToolCall {
                        name: tc.function.name.clone(),
                        arguments_json: tc.function.arguments.clone(),
                        tool_call_id: Some(tc.id.clone()),
                    })
                    .collect()
            } else {
                let parse_result = parse_tool_calls(&response.content.clone().unwrap_or_default());
                if parse_result.calls.is_empty() {
                    let display_text = response.content.clone().unwrap_or_default();

                    // QW-6: Conditional follow-through guardrail
                    if iterations < self.max_tool_iterations.saturating_sub(1)
                        && should_force_follow_through(&display_text, &self.tools)
                    {
                        // BREAK REPETITION LOOP: Check if we just sent this exact nudge 
                        // OR if the model is just repeating the exact same text.
                        let is_repeating = if self.history.len() >= 2 {
                            let prev_msg = &self.history[self.history.len() - 2];
                            // If history[len-1] is the current response (pushed above), 
                            // history[len-2] is the previous SYSTEM nudge or user message.
                            // If we want to check the PREVIOUS assistant message, it's history[len-3].
                            let prev_assistant = if self.history.len() >= 3 {
                                Some(&self.history[self.history.len() - 3])
                            } else {
                                None
                            };

                            let nudge_already_sent = prev_msg.role == "user" && prev_msg.content.contains("SYSTEM: You promised to take action now");
                            let text_repeated = prev_assistant.map(|m| m.role == "assistant" && m.content == display_text).unwrap_or(false);
                            
                            nudge_already_sent || text_repeated
                        } else {
                            false
                        };

                        if !is_repeating {
                            self.history.push(ChatMessage {
                                role: "user".to_string(),
                                content: "SYSTEM: You promised to take action now (e.g. \"I'll check now\" or \"Let me try\"). Do it in this turn by issuing the appropriate tool call(s). If no tool can perform it, explain the limitation clearly and do not promise a future attempt.".to_string(),
                                name: None,
                                tool_calls: None,
                                tool_call_id: None,
                                content_parts: None,
                            });
                            iterations += 1;
                            continue;
                        }
                    }

                    // No tool calls — return final text
                    return Ok(strip_tool_call_markup(&display_text));
                }
                parse_result.calls
            };

            let mut handles = Vec::new();
            for tool_call in &tool_calls_to_execute {
                let tool = self
                    .tools
                    .iter()
                    .find(|t| t.name() == tool_call.name)
                    .cloned();
                let call = tool_call.clone();
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
                                },
                            },
                            Err(e) => ToolExecutionResult {
                                name: call.name.clone(),
                                output: format!("Error parsing arguments: {}", e),
                                success: false,
                                tool_call_id: call.tool_call_id.clone(),
                            },
                        }
                    } else {
                        ToolExecutionResult {
                            name: call.name.clone(),
                            output: format!("Tool not found: {}", call.name),
                            success: false,
                            tool_call_id: call.tool_call_id.clone(),
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

            for result in execution_results {
                let formatted_output = if result.success {
                    result.output
                } else {
                    format!("❌ Tool Error ({}): {}", result.name, result.output)
                };

                self.history.push(ChatMessage {
                    role: "tool".to_string(),
                    content: formatted_output,
                    name: Some(result.name),
                    tool_calls: None,
                    tool_call_id: result.tool_call_id,
                    content_parts: None,
                });
            }

            // Add reflection prompt for the next iteration
            self.history.push(ChatMessage {
                role: "user".to_string(),
                content: "Reflect on the tool results above and decide your next steps. If a tool failed, try a different approach or fix the parameters. If you have enough info, provide a final answer.".to_string(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            });

            // ── CRITICAL: re-enter loop so the provider sees tool results ──
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
        assert_eq!(out, "\u{2705} Done.");
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
}
