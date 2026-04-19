use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent::Agent;
use crate::agent::memory_loader::Memory;
use crate::providers::Provider;
use crate::tools::{Tool, ToolContext};

/// Session wraps an Agent instance for an ongoing conversation.
/// It maintains the last active time and an internal async mutex to serialize turns
/// per session key.
pub struct Session {
    pub agent: tokio::sync::Mutex<Agent>,
    pub created_at: u64,
    pub last_active: AtomicU64,
    pub session_key: String,
    pub turn_count: AtomicU64,
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
}

impl SessionManager {
    pub fn new(
        config: Arc<crate::config::Config>,
        provider: Arc<dyn Provider>,
        tools: Vec<Arc<dyn Tool>>,
        memory: Option<Arc<dyn Memory>>,
    ) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            config,
            provider,
            tools,
            memory,
        }
    }

    /// Retrieve an existing session by its key, or create a new one.
    pub fn get_or_create(&self, session_key: &str, agent_id: &str) -> Arc<Session> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(session) = sessions.get(session_key) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            session.last_active.store(now, Ordering::SeqCst);
            return Arc::clone(session);
        }

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
            model,
            self.config.workspace_dir.clone(),
            Some(&self.config),
        );
        agent.config_path = if self.config.config_path.is_empty() {
            None
        } else {
            Some(self.config.config_path.clone())
        };

        if let Some(cfg) = agent_cfg
            && let Some(prompt) = &cfg.system_prompt
        {
            agent = agent.with_system_prompt(prompt);
        }

        if let Some(mem) = &self.memory {
            agent = agent.with_memory(Arc::clone(mem));
        }
        agent.memory_session_id = Some(session_key.to_string());

        let new_session = Arc::new(Session::new(agent, session_key.to_string()));
        sessions.insert(session_key.to_string(), Arc::clone(&new_session));

        new_session
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

    /// Safely process a message with a streaming callback for a specific session.
    pub async fn process_message_stream(
        &self,
        session_key: &str,
        agent_id: &str,
        message: String,
        context: ToolContext,
        callback: crate::providers::StreamCallback,
    ) -> Result<String> {
        let session_arc = self.get_or_create(session_key, agent_id);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        session_arc.last_active.store(now, Ordering::SeqCst);

        let mut agent_guard = session_arc.agent.lock().await;
        let response = agent_guard.turn_stream(message, &context, callback).await?;

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
