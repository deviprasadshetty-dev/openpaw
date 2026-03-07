pub mod commands;
pub mod compaction;
pub mod context_tokens;
pub mod dispatcher;
pub mod max_tokens;
pub mod memory_loader;
pub mod prompt;
pub mod routing;

pub use routing::{
    AgentBinding, BindingMatch, ChatType, MatchedBy, PeerRef, ResolvedRoute, RouteInput,
    resolve_route,
};

use anyhow::Result;
use regex::Regex;
use std::sync::Arc;
use tracing::warn;

use crate::providers::{ChatMessage, ChatRequest, Provider, ToolSpec};
use crate::tools::Tool;

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
}

/// Removes raw tool-call markup from LLM output that leaked into the final text.
/// Strips:
///   - `<tool_call>...</tool_call>` XML blocks
///   - `[tool_call]...[/tool_call]` and `[TOOL_CALL]...[/TOOL_CALL]` bracket blocks
///   - `` ```tool_call...``` `` fenced code blocks
///   - `` ```tool_code...``` `` fenced blocks that look like Python/JSON dumps
/// Returns a trimmed result; falls back to "\u{2705} Done." if everything was stripped.
pub fn strip_tool_call_markup(text: &str) -> String {
    // 1. XML-style <tool_call>...</tool_call>
    let xml_re = Regex::new(r"(?si)<tool_call>.*?</tool_call>").unwrap();
    let out = xml_re.replace_all(text, "");

    // 2. Bracket-style [tool_call]...[/tool_call] (case-insensitive)
    let bracket_re = Regex::new(r"(?si)\[tool_call\].*?\[/tool_call\]").unwrap();
    let out = bracket_re.replace_all(&out, "");

    // 3. Fenced ```tool_call ... ``` code blocks
    let fenced_tc_re = Regex::new(r"(?si)```\s*tool_call.*?```").unwrap();
    let out = fenced_tc_re.replace_all(&out, "");

    // 4. Fenced ```tool_code ... ``` blocks that look like Python/JSON dumps
    //    (heuristic: block starts with 'import json' or 'json.loads(')
    let fenced_py_re = Regex::new(r"(?si)```\s*tool_code\s*\nimport json.*?```").unwrap();
    let out = fenced_py_re.replace_all(&out, "");
    let fenced_py2_re = Regex::new(r"(?si)```\s*tool_code\s*\njson\.loads\(.*?```").unwrap();
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

        Self {
            provider,
            tools,
            memory: Arc::new(NoopMemory), // Default to no-op
            model_name,
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
            tool_cache: crate::tools::cache::ToolCache::new(30), // Default 30s TTL
            command_registry: commands::CommandRegistry::new(),
        }
    }

    pub fn with_memory(mut self, memory: Arc<dyn Memory>) -> Self {
        self.memory = memory;
        self
    }

    pub fn reset_history(&mut self) {
        self.history.clear();
        self.has_system_prompt = false;
        self.workspace_prompt_fingerprint = None;
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
        };

        Ok(prompt::build_system_prompt(ctx))
    }

    pub async fn turn(&mut self, user_message: String) -> Result<String> {
        // Handle Slash Commands
        if let Some(slash_response) = commands::CommandRegistry::handle_message(self, &user_message)
        {
            return Ok(slash_response);
        }

        // Auto-save user message
        if self.auto_save {
            // Placeholder: timestamp-based key
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

        // System Prompt Management
        let workspace_fp = prompt::workspace_prompt_fingerprint(&self.workspace_dir);
        if self.has_system_prompt && Some(workspace_fp) != self.workspace_prompt_fingerprint {
            self.has_system_prompt = false;
        }

        if !self.has_system_prompt {
            let system_prompt = self.build_system_prompt()?;

            // Insert or replace system prompt at index 0
            if !self.history.is_empty() && self.history[0].role == "system" {
                self.history[0].content = system_prompt;
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
            self.workspace_prompt_fingerprint = Some(workspace_fp);
        }

        // Enrich user message with memory context
        let enriched_msg = match memory_loader::enrich_message(
            self.memory.as_ref(),
            &user_message,
            self.memory_session_id.as_deref(),
        ) {
            Ok(msg) => msg,
            Err(e) => {
                warn!("Memory enrichment failed: {}", e);
                user_message.clone()
            }
        };

        // Append user message
        self.history.push(ChatMessage {
            role: "user".to_string(),
            content: enriched_msg,
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

            // We ignore the result (bool) as it just indicates if compaction happened
            let _ = auto_compact_history(
                &mut self.history,
                &self.provider,
                &self.model_name,
                &compaction_config,
            );
        }

        // ── Follow-through detection (mirrors Nullclaw) ────────────────────────
        /// Returns true when the model promises action ("I'll try"/"Let me check")
        /// but hasn't actually issued a tool call.
        fn should_force_follow_through(text: &str) -> bool {
            let lower = text.to_lowercase();
            const PATTERNS: &[&str] = &[
                "i'll try",
                "i will try",
                "let me try",
                "i'll check",
                "i will check",
                "let me check",
                "i'll retry",
                "i will retry",
                "let me retry",
                "i'll attempt",
                "i will attempt",
                "i'll do that now",
                "i will do that now",
                "doing that now",
                "let me do that",
                "i'll look",
                "i will look",
                "let me look",
            ];
            PATTERNS.iter().any(|p| lower.contains(p))
        }

        // Loop for tools
        let mut iterations = 0;
        loop {
            if iterations >= self.max_tool_iterations {
                warn!("Max tool iterations reached");
                break;
            }

            // Prepare ToolSpecs
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

            let request = ChatRequest {
                messages: &self.history,
                model: &self.model_name,
                temperature: self.temperature,
                max_tokens: Some(self.max_tokens),
                tools: if tool_specs.is_empty() {
                    None
                } else {
                    Some(&tool_specs)
                },
                timeout_secs: 60,
                reasoning_effort: None,
            };

            let response = self.provider.chat(&request)?;

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

            // Handle tool calls
            // For providers with native tool support, use response.tool_calls
            // For others, parse tool calls from the response content
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

                    // ── Action follow-through guardrail ──────────────────
                    // If the model promised to act but issued no tool call,
                    // inject a SYSTEM message and force another iteration.
                    if iterations < self.max_tool_iterations.saturating_sub(1)
                        && should_force_follow_through(&display_text)
                    {
                        self.history.push(ChatMessage {
                            role: "assistant".to_string(),
                            content: display_text,
                            name: None,
                            tool_calls: None,
                            tool_call_id: None,
                            content_parts: None,
                        });
                        self.history.push(ChatMessage {
                            role: "user".to_string(),
                            content: "SYSTEM: You promised to take action now (e.g. \"I'll try/check now\"). \
                                Issue the appropriate tool call(s) in this turn. \
                                If no tool can do it, state the limitation clearly — do not defer again."
                                .to_string(),
                            name: None,
                            tool_calls: None,
                            tool_call_id: None,
                            content_parts: None,
                        });
                        iterations += 1;
                        continue;
                    }

                    // No tool calls, no promise — genuine final response
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
                let handle = tokio::task::spawn_blocking(move || {
                    if let Some(tool) = tool {
                        if tool.cacheable() {
                            if let Some(cached_result) = cache.get(&call.name, &call.arguments_json)
                            {
                                let mut res = cached_result;
                                res.tool_call_id = call.tool_call_id.clone();
                                return res;
                            }
                        }

                        match serde_json::from_str::<serde_json::Value>(&call.arguments_json) {
                            Ok(args) => match tool.execute(args) {
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

            // Append tool results to history
            for result in execution_results {
                self.history.push(ChatMessage {
                    role: "tool".to_string(),
                    content: result.output,
                    name: Some(result.name),
                    tool_calls: None,
                    tool_call_id: result.tool_call_id,
                    content_parts: None,
                });
            }

            // ── CRITICAL: re-enter loop so the provider sees tool results ──
            iterations += 1;
            continue;
        }

        Ok(strip_tool_call_markup(
            "[Agent]: Max tool iterations reached",
        ))
    }

    /// Streaming variant of `turn()`. Emits text deltas to `callback` as tokens arrive.
    /// Returns the final complete response text (same as `turn()`).
    pub async fn turn_stream(
        &mut self,
        user_message: String,
        callback: crate::providers::StreamCallback,
    ) -> Result<String> {
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

        // Enrich user message with memory context
        let enriched_msg = match memory_loader::enrich_message(
            self.memory.as_ref(),
            &user_message,
            self.memory_session_id.as_deref(),
        ) {
            Ok(msg) => msg,
            Err(e) => {
                warn!("Memory enrichment failed: {}", e);
                user_message.clone()
            }
        };

        // Auto-save user message (un-enriched original)
        if self.auto_save {
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

        // System prompt management (same as turn())
        let workspace_fp = prompt::workspace_prompt_fingerprint(&self.workspace_dir);
        if self.has_system_prompt && Some(workspace_fp) != self.workspace_prompt_fingerprint {
            self.has_system_prompt = false;
        }

        if !self.has_system_prompt {
            let system_prompt = self.build_system_prompt()?;
            if !self.history.is_empty() && self.history[0].role == "system" {
                self.history[0].content = system_prompt;
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
            self.workspace_prompt_fingerprint = Some(workspace_fp);
        }

        // Append user message
        self.history.push(ChatMessage {
            role: "user".to_string(),
            content: enriched_msg,
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

        // Tool loop with streaming
        let mut iterations = 0;
        loop {
            if iterations >= self.max_tool_iterations {
                warn!("Max tool iterations reached");
                break;
            }

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

            let request = ChatRequest {
                messages: &self.history,
                model: &self.model_name,
                temperature: self.temperature,
                max_tokens: Some(self.max_tokens),
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
            let acc_clone = accumulated.clone();
            let cb_clone = shared_callback.clone();

            // Create a forwarding callback that both accumulates and forwards deltas
            let stream_cb: crate::providers::StreamCallback =
                Box::new(move |chunk: StreamChunk| {
                    if let StreamChunk::Delta(ref text) = chunk {
                        if let Ok(mut acc) = acc_clone.lock() {
                            acc.push_str(text);
                        }
                    }
                    if let Ok(mut cb) = cb_clone.lock() {
                        cb(chunk);
                    }
                });

            let response = self.provider.chat_stream(&request, stream_cb)?;

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
                    // No tool calls — return final text
                    return Ok(strip_tool_call_markup(
                        &response.content.unwrap_or_default(),
                    ));
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
                let handle = tokio::task::spawn_blocking(move || {
                    if let Some(tool) = tool {
                        if tool.cacheable() {
                            if let Some(cached_result) = cache.get(&call.name, &call.arguments_json)
                            {
                                let mut res = cached_result;
                                res.tool_call_id = call.tool_call_id.clone();
                                return res;
                            }
                        }

                        match serde_json::from_str::<serde_json::Value>(&call.arguments_json) {
                            Ok(args) => match tool.execute(args) {
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
                self.history.push(ChatMessage {
                    role: "tool".to_string(),
                    content: result.output,
                    name: Some(result.name),
                    tool_calls: None,
                    tool_call_id: result.tool_call_id,
                    content_parts: None,
                });
            }

            // ── CRITICAL: re-enter loop so the provider sees tool results ──
            iterations += 1;
            continue;
        }

        Ok(strip_tool_call_markup(
            "[Agent]: Max tool iterations reached",
        ))
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
}
