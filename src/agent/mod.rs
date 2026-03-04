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
    pub auto_save: bool,
    pub has_system_prompt: bool,
    pub workspace_prompt_fingerprint: Option<u64>,
    pub memory_session_id: Option<String>,
    pub history: Vec<ChatMessage>,
    pub total_tokens: u64,
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
            workspace_dir,
            auto_save: true,
            has_system_prompt: false,
            workspace_prompt_fingerprint: None,
            memory_session_id: None,
            history: Vec::new(),
            total_tokens: 0,
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
        if let Some(slash_response) = commands::handle_slash_command(self, &user_message) {
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

        // Append user message
        self.history.push(ChatMessage {
            role: "user".to_string(),
            content: user_message,
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
                    return Ok(display_text);
                }
                parse_result.calls
            };

            let mut execution_results = Vec::new();
            for tool_call in &tool_calls_to_execute {
                let tool = self.tools.iter().find(|t| t.name() == tool_call.name);
                let result = if let Some(tool) = tool {
                    match serde_json::from_str::<serde_json::Value>(&tool_call.arguments_json) {
                        Ok(args) => match tool.execute(args) {
                            Ok(output) => ToolExecutionResult {
                                name: tool_call.name.clone(),
                                output: output.content,
                                success: true,
                                tool_call_id: tool_call.tool_call_id.clone(),
                            },
                            Err(e) => ToolExecutionResult {
                                name: tool_call.name.clone(),
                                output: format!("Error executing tool: {}", e),
                                success: false,
                                tool_call_id: tool_call.tool_call_id.clone(),
                            },
                        },
                        Err(e) => ToolExecutionResult {
                            name: tool_call.name.clone(),
                            output: format!("Error parsing arguments: {}", e),
                            success: false,
                            tool_call_id: tool_call.tool_call_id.clone(),
                        },
                    }
                } else {
                    ToolExecutionResult {
                        name: tool_call.name.clone(),
                        output: format!("Tool not found: {}", tool_call.name),
                        success: false,
                        tool_call_id: tool_call.tool_call_id.clone(),
                    }
                };
                execution_results.push(result);
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

        Ok("[Agent]: Max tool iterations reached".to_string())
    }
}
