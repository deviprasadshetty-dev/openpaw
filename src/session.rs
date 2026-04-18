use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

use crate::agent::Agent;
use crate::agent::memory_loader::Memory;
use crate::agent::prompt::ConversationContext;
use crate::providers::Provider;
use crate::tools::{Tool, ToolContext};

pub struct Session {
    pub agent: tokio::sync::Mutex<Agent>,
    pub created_at: u64,
    pub last_active: AtomicU64,
    pub session_key: String,
    pub turn_count: AtomicU64,
    pub cancel_token: tokio::sync::Mutex<CancellationToken>,
}

impl Session {
    pub fn new(agent: Agent, session_key: String) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            agent: tokio::sync::Mutex::new(agent),
            created_at: now,
            last_active: AtomicU64::new(now),
            session_key,
            turn_count: AtomicU64::new(0),
            cancel_token: tokio::sync::Mutex::new(CancellationToken::new()),
        }
    }
}

/// SessionManager manages a collection of persistent in-process Agent sessions.
/// It creates lazy agents on demand and cleans up idle ones.
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    // Shared state/config
    config: Arc<crate::config::Config>,
    provider: Arc<dyn Provider>,
    tools: Vec<Arc<dyn Tool>>,
    memory: Option<Arc<dyn Memory>>,
    pub max_sessions: usize,
}

impl SessionManager {
    pub fn new(
        config: Arc<crate::config::Config>,
        provider: Arc<dyn Provider>,
        tools: Vec<Arc<dyn Tool>>,
        memory: Option<Arc<dyn Memory>>,
    ) -> Self {
        let max_sessions = config.session.max_sessions.unwrap_or(1000);
        Self {
            sessions: Mutex::new(HashMap::new()),
            config,
            provider,
            tools,
            memory,
            max_sessions,
        }
    }

    /// Retrieve an existing session by its key, or create a new one.
    pub fn get_or_create(&self, session_key: &str, agent_id: &str) -> Arc<Session> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());

        // Evict if over capacity (and we are not already in it)
        if !sessions.contains_key(session_key) && sessions.len() >= self.max_sessions {
            let mut oldest_key = None;
            let mut oldest_time = u64::MAX;
            for (k, v) in sessions.iter() {
                let last_active = v.last_active.load(Ordering::SeqCst);
                if last_active < oldest_time {
                    oldest_time = last_active;
                    oldest_key = Some(k.clone());
                }
            }
            if let Some(k) = oldest_key {
                sessions.remove(&k);
            }
        }

        let entry = sessions.entry(session_key.to_string()).or_insert_with(|| {
            // Look up agent config
            let agent_cfg = self.config.agents.iter().find(|a| a.name == agent_id);

            let provider = Arc::clone(&self.provider);
            let model = agent_cfg.map(|a| a.model.clone()).unwrap_or_else(|| {
                self.config
                    .get_model_for_provider(&self.config.default_provider)
                    .unwrap_or_else(|| "gpt-4o".to_string())
            });

            // Create new agent with the shared provider, tools, memory, etc.
            let mut agent = Agent::new(
                provider,
                self.tools.clone(),
                model.clone(),
                self.config.workspace_dir.clone(),
            );
            agent.config_path = if self.config.config_path.is_empty() {
                None
            } else {
                Some(self.config.config_path.clone())
            };

            // Apply task-based model routing from config
            let task_config = crate::model_router::TaskModelConfig::with_overrides(
                &model,
                &self.config.task_models.to_map(),
            );
            agent = agent.with_task_models(&task_config);

            if let Some(cfg) = agent_cfg
                && let Some(prompt) = &cfg.system_prompt {
                    agent = agent.with_system_prompt(prompt);
                }

            if let Some(mem) = &self.memory {
                agent = agent.with_memory(Arc::clone(mem));
            }
            agent.memory_session_id = Some(session_key.to_string());

            Arc::new(Session::new(agent, session_key.to_string()))
        });

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        entry.last_active.store(now, Ordering::SeqCst);
        Arc::clone(entry)
    }

    /// Safely process a message for a specific session.
    pub async fn process_message(
        &self,
        session_key: &str,
        agent_id: &str,
        message: String,
        context: ToolContext,
    ) -> Result<String> {
        let session_arc = self.get_or_create(session_key, agent_id);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        session_arc.last_active.store(now, Ordering::SeqCst);

        let mut agent_guard = session_arc.agent.lock().await;
        let response = agent_guard.turn(message, &context).await?;

        session_arc.turn_count.fetch_add(1, Ordering::SeqCst);
        session_arc.last_active.store(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Ordering::SeqCst,
        );

        Ok(response)
    }

    /// Cancel any in-progress turn for the given session key.
    /// Returns true if a session was found (and its token was cancelled).
    pub async fn cancel_turn(&self, session_key: &str) -> bool {
        let session = {
            let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            sessions.get(session_key).map(Arc::clone)
        };

        if let Some(session) = session {
            let token = session.cancel_token.lock().await;
            token.cancel();
            true
        } else {
            false
        }
    }

    /// Safely process a message with a streaming callback for a specific session.
    /// Resets the cancellation token before starting so previous cancellations
    /// don't block new turns.
    /// `conversation_context` is set on the agent before processing so the
    /// system prompt includes channel-aware context (e.g. Telegram group chat).
    pub async fn process_message_stream(
        &self,
        session_key: &str,
        agent_id: &str,
        message: String,
        context: ToolContext,
        callback: crate::providers::StreamCallback,
        conversation_context: Option<ConversationContext>,
    ) -> Result<String> {
        let session_arc = self.get_or_create(session_key, agent_id);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        session_arc.last_active.store(now, Ordering::SeqCst);

        // Reset cancellation token so a new turn can start even if the
        // previous one was cancelled with /stop.
        let cancel_token = {
            let mut token = session_arc.cancel_token.lock().await;
            *token = CancellationToken::new();
            token.clone()
        };

        let mut agent_guard = session_arc.agent.lock().await;
        if let Some(cc) = conversation_context {
            agent_guard.conversation_context = Some(cc);
        }
        let response = agent_guard
            .turn_stream(message, &context, callback, Some(cancel_token))
            .await?;

        session_arc.turn_count.fetch_add(1, Ordering::SeqCst);
        session_arc.last_active.store(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Ordering::SeqCst,
        );

        Ok(response)
    }

    /// Evict sessions idle longer than `max_idle_secs`. Returns the number evicted.
    pub fn evict_idle(&self, max_idle_secs: u64) -> usize {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Collect keys to remove
        let mut to_remove = Vec::new();
        for (k, v) in sessions.iter() {
            let active = v.last_active.load(Ordering::SeqCst);
            if active + max_idle_secs < now {
                to_remove.push(k.clone());
            }
        }

        let evicted_count = to_remove.len();
        for key in to_remove {
            sessions.remove(&key);
        }

        evicted_count
    }

    pub fn session_count(&self) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    pub fn get_config(&self) -> &crate::config::Config {
        &self.config
    }
}
