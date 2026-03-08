use crate::bus::Bus;
use crate::session::SessionManager;
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub struct TaskState {
    pub status: TaskStatus,
    pub label: String,
    pub session_key: Option<String>,
    pub result: Option<String>,
    pub error_msg: Option<String>,
    pub started_at: u64,
    pub completed_at: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct SubagentConfig {
    pub max_iterations: u32,
    pub max_concurrent: u32,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 15,
            max_concurrent: 4,
        }
    }
}

pub struct SubagentManager {
    tasks: Arc<Mutex<HashMap<u64, TaskState>>>,
    next_id: Arc<Mutex<u64>>,
    config: SubagentConfig,
    bus: Arc<Bus>,
    session_manager: Arc<Mutex<Option<Arc<SessionManager>>>>,
}

impl SubagentManager {
    pub fn new(bus: Arc<Bus>, subagent_config: SubagentConfig) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            config: subagent_config,
            bus,
            session_manager: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_session_manager(&self, sm: Arc<SessionManager>) {
        let mut guard = self
            .session_manager
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *guard = Some(sm);
    }

    pub fn spawn(
        &self,
        task: &str,
        label: &str,
        origin_channel: &str,
        origin_chat_id: &str,
    ) -> Result<u64> {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        let running_count = tasks
            .values()
            .filter(|t| t.status == TaskStatus::Running)
            .count();

        if running_count >= self.config.max_concurrent as usize {
            return Err(anyhow!("Too many concurrent subagents"));
        }

        let mut next_id = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
        let task_id = *next_id;
        *next_id += 1;

        let subagent_session_key = format!("subagent:{}", task_id);
        let state = TaskState {
            status: TaskStatus::Running,
            label: label.to_string(),
            session_key: Some(subagent_session_key.clone()),
            result: None,
            error_msg: None,
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            completed_at: None,
        };

        tasks.insert(task_id, state);

        let task_copy = task.to_string();
        let label_copy = label.to_string();
        let origin_channel_copy = origin_channel.to_string();
        let origin_chat_copy = origin_chat_id.to_string();

        let tasks_clone = self.tasks.clone();
        let sm_opt_clone = self.session_manager.clone();
        let bus_clone = self.bus.clone();

        // Spawn asynchronous task
        tokio::spawn(async move {
            let sm = {
                let guard = sm_opt_clone.lock().unwrap_or_else(|e| e.into_inner());
                guard.clone()
            };

            if let Some(sm) = sm {
                let context = crate::tools::root::ToolContext {
                    channel: origin_channel_copy.clone(),
                    sender_id: "subagent".to_string(),
                    chat_id: origin_chat_copy.clone(),
                    session_key: subagent_session_key.clone(),
                };
                let result = sm.process_message(&subagent_session_key, task_copy, context).await;

                let mut tasks = tasks_clone.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(state) = tasks.get_mut(&task_id) {
                    match result {
                        Ok(response) => {
                            state.status = TaskStatus::Completed;
                            state.result = Some(response.clone());
                            state.completed_at = Some(
                                SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs(),
                            );

                            // Report back to origin via Bus
                            let report = format!(
                                "✅ Subagent '{}' (ID {}) completed:\n\n{}",
                                label_copy, task_id, response
                            );
                            let outbound = crate::bus::make_outbound(
                                &origin_channel_copy,
                                &origin_chat_copy,
                                &report,
                            );
                            let _ = bus_clone.publish_outbound(outbound);
                        }
                        Err(e) => {
                            state.status = TaskStatus::Failed;
                            state.error_msg = Some(e.to_string());
                            state.completed_at = Some(
                                SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs(),
                            );

                            // Report error
                            let report = format!(
                                "❌ Subagent '{}' (ID {}) failed: {}",
                                label_copy, task_id, e
                            );
                            let outbound = crate::bus::make_outbound(
                                &origin_channel_copy,
                                &origin_chat_copy,
                                &report,
                            );
                            let _ = bus_clone.publish_outbound(outbound);
                        }
                    }
                }
            } else {
                let mut tasks = tasks_clone.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(state) = tasks.get_mut(&task_id) {
                    state.status = TaskStatus::Failed;
                    state.error_msg =
                        Some("Session manager not initialized in subagent manager".to_string());
                }
            }
        });

        Ok(task_id)
    }

    pub fn get_task_status(&self, task_id: u64) -> Option<TaskStatus> {
        let tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        tasks.get(&task_id).map(|t| t.status)
    }

    pub fn get_task_result(&self, task_id: u64) -> Option<String> {
        let tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        tasks.get(&task_id).and_then(|t| t.result.clone())
    }
}
