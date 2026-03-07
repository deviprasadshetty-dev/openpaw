use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent::Agent;
use crate::agent::memory_loader::Memory;
use crate::providers::Provider;
use crate::tools::Tool;

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
    // Below are factories or shared instances we inject into new Agents.
    // We clone these Arcs whenever we spawn a new session.
    provider: Arc<dyn Provider>,
    tools: Vec<Arc<dyn Tool>>,
    memory: Option<Arc<dyn Memory>>,
    model_name: String,
    workspace_dir: String,
}

impl SessionManager {
    pub fn new(
        provider: Arc<dyn Provider>,
        tools: Vec<Arc<dyn Tool>>,
        memory: Option<Arc<dyn Memory>>,
        model_name: String,
        workspace_dir: String,
    ) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            provider,
            tools,
            memory,
            model_name,
            workspace_dir,
        }
    }

    /// Retrieve an existing session by its key, or create a new one.
    pub fn get_or_create(&self, session_key: &str) -> Arc<Session> {
        let mut sessions = self.sessions.lock().unwrap();

        if let Some(session) = sessions.get(session_key) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            session.last_active.store(now, Ordering::SeqCst);
            return Arc::clone(session);
        }

        // Create new agent with the shared provider, tools, memory, etc.
        let mut agent = Agent::new(
            Arc::clone(&self.provider),
            self.tools.clone(),
            self.model_name.clone(),
            self.workspace_dir.clone(),
        );

        if let Some(mem) = &self.memory {
            agent = agent.with_memory(Arc::clone(mem));
        }
        agent.memory_session_id = Some(session_key.to_string());

        let new_session = Arc::new(Session::new(agent, session_key.to_string()));
        sessions.insert(session_key.to_string(), Arc::clone(&new_session));

        new_session
    }

    /// Safely process a message for a specific session.
    pub async fn process_message(&self, session_key: &str, message: String) -> Result<String> {
        let session_arc = self.get_or_create(session_key);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        session_arc.last_active.store(now, Ordering::SeqCst);

        let mut agent_guard = session_arc.agent.lock().await;
        let response = agent_guard.turn(message).await?;

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
        message: String,
        callback: crate::providers::StreamCallback,
    ) -> Result<String> {
        let session_arc = self.get_or_create(session_key);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        session_arc.last_active.store(now, Ordering::SeqCst);

        let mut agent_guard = session_arc.agent.lock().await;
        let response = agent_guard.turn_stream(message, callback).await?;

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
        let mut sessions = self.sessions.lock().unwrap();
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
        self.sessions.lock().unwrap().len()
    }
}
