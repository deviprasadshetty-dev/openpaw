use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn, error};

use crate::config::Config;
use crate::daemon::DaemonState;
use crate::memory::MemoryCategory;
use crate::providers::{ChatMessage, ChatRequest, Provider};

const IDLE_THRESHOLD_SECS: i64 = 15 * 60; // 15 minutes
const DREAM_COOLDOWN_SECS: i64 = 4 * 60 * 60; // 4 hours
const MEMORY_CHUNK_SIZE: usize = 50;

pub fn dream_thread(
    state: Arc<std::sync::Mutex<DaemonState>>,
    config: Arc<Config>,
    provider: Arc<dyn Provider>,
    memory: Option<Arc<dyn crate::agent::memory_loader::Memory>>,
) {
    info!("Dream thread started");
    loop {
        if crate::daemon::is_shutdown_requested() {
            break;
        }

        std::thread::sleep(Duration::from_secs(60)); // Poll every minute

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let should_dream = {
            let guard = state.lock().unwrap();
            let idle_time = now - guard.last_activity_at;
            let time_since_dream = now - guard.last_dream_at;
            idle_time >= IDLE_THRESHOLD_SECS && time_since_dream >= DREAM_COOLDOWN_SECS
        };

        if should_dream {
            info!("System is idle. Triggering Dream Sequence...");
            
            if let Some(mem) = &memory {
                let model = config.default_model.clone().unwrap_or_else(|| "default".to_string());
                match dream_sequence(provider.clone(), mem.clone(), &model) {
                    Ok(_) => {
                        info!("Dream Sequence completed successfully.");
                        let mut guard = state.lock().unwrap();
                        guard.last_dream_at = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;
                    }
                    Err(e) => {
                        error!("Dream Sequence failed: {}", e);
                    }
                }
            } else {
                warn!("No memory backend available for dreaming.");
            }
        }
    }
}

fn dream_sequence(
    provider: Arc<dyn Provider>,
    memory: Arc<dyn crate::agent::memory_loader::Memory>,
    model_name: &str,
) -> anyhow::Result<()> {
    // 1. Fetch recent memories that aren't learnings
    let recent_memories = memory.get_recent(MEMORY_CHUNK_SIZE)?;
    
    if recent_memories.is_empty() {
        return Ok(()); // Nothing to dream about
    }

    let mut memory_texts = Vec::new();
    for mem in &recent_memories {
        memory_texts.push(format!("ID: {}\nContent: {}", mem.id, mem.content));
    }

    let joined_memories = memory_texts.join("\n---\n");

    // 2. LLM Prompt
    let system_prompt = "You are the Dream Subconscious of OpenPaw.
Your task is to review recent memories, find patterns, condense redundant information, prune useless details, and extract core 'Learnings' (e.g. user preferences, workflow tips, tool usage nuances).

Output format MUST be strict JSON:
{
  \"delete_ids\": [\"id1\", \"id2\"],
  \"new_learnings\": [
    \"User prefers concise answers\",
    \"Tool xyz requires the --force flag for this specific error\"
  ]
}";
    
    let user_prompt = format!("Review the following memories and provide JSON:\n\n{}", joined_memories);

    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: system_prompt.to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
            thought_signature: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: user_prompt,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
            thought_signature: None,
        },
    ];

    let model_owned = model_name.to_string();
    let req = ChatRequest {
        messages: &messages,
        model: &model_owned,
        temperature: 0.3,
        max_tokens: Some(1024),
        tools: None,
        timeout_secs: 120,
        reasoning_effort: None,
    };

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let response = rt.block_on(async { provider.chat(&req) })?;
    
    // Parse response
    let content_opt = response.content;
    let content_str = content_opt.unwrap_or_default();
    let content = content_str.trim();
    
    // Strip markdown JSON blocks if present
    let content = if content.starts_with("```json") {
        content.trim_start_matches("```json").trim_end_matches("```").trim()
    } else {
        content
    };
    
    #[derive(serde::Deserialize)]
    struct DreamResult {
        delete_ids: Vec<String>,
        new_learnings: Vec<String>,
    }

    let result: DreamResult = serde_json::from_str(content)?;

    // 3. Memory Updates
    for id in result.delete_ids {
        let _ = memory.forget_by_id(&id);
    }

    for learning in result.new_learnings {
        let key = format!("learning_{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos());
        let _ = memory.store_with_category(
            &key,
            &learning,
            MemoryCategory::Learning,
            None,
            Some(0.8) // High importance
        );
    }

    Ok(())
}
