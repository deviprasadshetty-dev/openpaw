use crate::providers::ChatMessage;
use crate::tools::{Tool, ToolContext};
use std::sync::Arc;

const MEMORY_REVIEW_PROMPT: &str = r#"Review the conversation above and consider saving to memory if appropriate.

Focus on:
1. Has the user revealed things about themselves — their persona, desires, preferences, or personal details worth remembering?
2. Has the user expressed expectations about how you should behave, their work style, or ways they want you to operate?

If something stands out, save it using the memory_md tool.
If nothing is worth saving, just say "Nothing to save." and stop."#;

const SKILL_REVIEW_PROMPT: &str = r#"Review the conversation above and consider saving or updating a skill if appropriate.

Focus on: was a non-trivial approach used to complete a task that required trial and error, or changing course due to experiential findings along the way, or did the user expect or desire a different method or outcome?

If a relevant skill already exists, update it with what you learned.
Otherwise, create a new skill if the approach is reusable.
If nothing is worth saving, just say "Nothing to save." and stop."#;

const COMBINED_REVIEW_PROMPT: &str = r#"Review the conversation above and consider two things:

**Memory**: Has the user revealed things about themselves — their persona, desires, preferences, or personal details worth remembering?
**Skills**: Was a non-trivial approach used to complete a task that required trial and error, or changing course due to experiential findings along the way?

Only act if there's something genuinely worth saving.
If nothing stands out, just say "Nothing to save." and stop."#;

const FLUSH_PROMPT: &str = r#"[System: The session is being compressed. Save anything worth remembering — prioritize user preferences, corrections, and recurring patterns over task-specific details.]

Use the memory_md tool to append important findings to the memory or user files."#;

/// Sentinel marker appended to history before flush and stripped afterward.
const FLUSH_SENTINEL: &str = "[FLUSH_SENTINEL]";

/// Spawn a background review that runs in a tokio task.
/// It creates a lightweight forked Agent with only memory/skill tools and
/// `max_tool_iterations=8` (Hermes-style), preventing recursive review spawning.
pub fn spawn_background_review(
    provider: Arc<dyn crate::providers::Provider>,
    cheap_provider: Option<Arc<dyn crate::providers::Provider>>,
    model_name: String,
    tools: Vec<Arc<dyn Tool>>,
    workspace_dir: String,
    conversation_history: Vec<ChatMessage>,
    review_memory: bool,
    review_skills: bool,
) {
    let prompt = if review_memory && review_skills {
        COMBINED_REVIEW_PROMPT
    } else if review_memory {
        MEMORY_REVIEW_PROMPT
    } else {
        SKILL_REVIEW_PROMPT
    };

    tokio::spawn(async move {
        // Filter to only memory and skill tools for the review
        let review_tools: Vec<Arc<dyn Tool>> = tools
            .into_iter()
            .filter(|t| {
                let n = t.name();
                n == "memory_md" || n == "skill_manage" || n == "memory_store"
            })
            .collect();

        if review_tools.is_empty() {
            return;
        }

        // Hermes-style: create a forked Agent with limited scope
        let mut agent = crate::agent::Agent::new(
            provider,
            cheap_provider,
            review_tools,
            model_name,
            workspace_dir,
        );
        agent.max_tool_iterations = 8;
        agent.memory_nudge_interval = 0;
        agent.skill_nudge_interval = 0;
        agent.auto_save = false;
        agent.history = conversation_history;

        let ctx = ToolContext {
            channel: "background_review".to_string(),
            sender_id: "system".to_string(),
            chat_id: "review".to_string(),
            session_key: "review".to_string(),
        };

        let _ = agent.turn(prompt.to_string(), &ctx).await;

        // Scan the review agent's history for successfully executed tool calls
        let mut summaries = Vec::new();
        let mut in_review = false;
        for msg in &agent.history {
            if msg.role == "user" && msg.content == prompt {
                in_review = true;
                continue;
            }
            if !in_review {
                continue;
            }
            if msg.role == "assistant" {
                if let Some(ref tool_calls) = msg.tool_calls {
                    for tc in tool_calls {
                        summaries.push(format!("Called {}", tc.function.name));
                    }
                }
            }
        }

        if !summaries.is_empty() {
            tracing::info!("Background review: {}", summaries.join(" · "));
        }
    });
}

/// Flush memories on exit by running a single-turn memory consolidation.
/// Appends a sentinel to `history`, makes one API call with only memory tools,
/// executes any tool calls, then strips the sentinel and all flush artifacts.
pub async fn flush_memories(
    provider: Arc<dyn crate::providers::Provider>,
    model_name: String,
    tools: Vec<Arc<dyn Tool>>,
    history: &mut Vec<ChatMessage>,
) {
    let memory_tools: Vec<Arc<dyn Tool>> = tools
        .into_iter()
        .filter(|t| {
            let n = t.name();
            n == "memory_md" || n == "memory_store"
        })
        .collect();

    if memory_tools.is_empty() {
        return;
    }

    // Record the index before the sentinel so we can strip afterward
    let pre_flush_len = history.len();

    // Append sentinel marker
    history.push(ChatMessage::user(format!(
        "{} {}",
        FLUSH_SENTINEL, FLUSH_PROMPT
    )));

    let tool_specs: Vec<crate::providers::ToolSpec> = memory_tools
        .iter()
        .map(|t| crate::providers::ToolSpec {
            name: t.name().to_string(),
            description: t.description().to_string(),
            parameters: serde_json::from_str(&t.parameters_json()).unwrap_or(serde_json::json!({})),
        })
        .collect();

    let request = crate::providers::ChatRequest {
        messages: history,
        model: &model_name,
        temperature: 0.3,
        max_tokens: Some(2000),
        tools: if tool_specs.is_empty() { None } else { Some(&tool_specs) },
        timeout_secs: 60,
        reasoning_effort: None,
    };

    let response = match provider.chat(&request) {
        Ok(r) => r,
        Err(_) => {
            history.truncate(pre_flush_len);
            return;
        }
    };

    // Execute any tool calls returned (single-turn, no loop)
    if !response.tool_calls.is_empty() {
        let ctx = ToolContext {
            channel: "flush".to_string(),
            sender_id: "system".to_string(),
            chat_id: "flush".to_string(),
            session_key: "flush".to_string(),
        };

        for tc in &response.tool_calls {
            if let Some(tool) = memory_tools.iter().find(|t| t.name() == tc.function.name) {
                let args = match serde_json::from_str::<serde_json::Value>(&tc.function.arguments) {
                    Ok(a) => a,
                    Err(_) => continue,
                };
                let _ = tool.execute(args, &ctx).await;
            }
        }
    }

    // Strip sentinel and all flush artifacts from history
    history.truncate(pre_flush_len);
}
