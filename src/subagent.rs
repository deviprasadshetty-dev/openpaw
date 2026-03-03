use crate::bus::Bus;
use crate::config::{Config, NamedAgentConfig};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
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
    // Thread handle handling is tricky in Rust structs if we want to clone/serialize state
    // We'll keep it simple for now or use Arc<Mutex<Option<JoinHandle<()>>>> if needed, 
    // but usually we just let it detach or keep handle separately. 
    // For this implementation, we won't store the handle in TaskState to avoid complexity.
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
    bus: Option<Arc<Bus>>,
    
    // Context
    api_key: Option<String>,
    default_provider: String,
    default_model: Option<String>,
    workspace_dir: String,
    agents: Vec<NamedAgentConfig>,
    http_enabled: bool,
}

impl SubagentManager {
    pub fn new(
        config: &Config,
        bus: Option<Arc<Bus>>,
        subagent_config: SubagentConfig,
    ) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            config: subagent_config,
            bus,
            api_key: config.default_provider_key(),
            default_provider: config.default_provider.clone(),
            default_model: config.default_model.clone(),
            workspace_dir: config.workspace_dir.clone(),
            agents: config.agents.clone(),
            http_enabled: config.http_request.enabled,
        }
    }

    pub fn spawn(
        &self,
        task: &str,
        label: &str,
        origin_channel: &str,
        origin_chat_id: &str,
    ) -> Result<u64> {
        let mut tasks = self.tasks.lock().unwrap();
        let running_count = tasks.values().filter(|t| t.status == TaskStatus::Running).count();
        
        if running_count >= self.config.max_concurrent as usize {
            return Err(anyhow!("Too many concurrent subagents"));
        }

        let mut next_id = self.next_id.lock().unwrap();
        let task_id = *next_id;
        *next_id += 1;

        let state = TaskState {
            status: TaskStatus::Running,
            label: label.to_string(),
            session_key: Some(origin_chat_id.to_string()),
            result: None,
            error_msg: None,
            started_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            completed_at: None,
        };

        tasks.insert(task_id, state);

        let task_copy = task.to_string();
        let _label_copy = label.to_string();
        let _origin_channel_copy = origin_channel.to_string();
        let _origin_chat_copy = origin_chat_id.to_string();
        
        let tasks_clone = self.tasks.clone();
        
        // Spawn thread
        thread::spawn(move || {
            // Actual subagent execution logic would go here
            // For now, we simulate execution
            // thread::sleep(std::time::Duration::from_secs(2));
            
            // On completion:
            let mut tasks = tasks_clone.lock().unwrap();
            if let Some(state) = tasks.get_mut(&task_id) {
                state.status = TaskStatus::Completed;
                state.result = Some(format!("Executed task: {}", task_copy));
                state.completed_at = Some(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs());
            }
        });

        Ok(task_id)
    }

    pub fn get_task_status(&self, task_id: u64) -> Option<TaskStatus> {
        let tasks = self.tasks.lock().unwrap();
        tasks.get(&task_id).map(|t| t.status)
    }

    pub fn get_task_result(&self, task_id: u64) -> Option<String> {
        let tasks = self.tasks.lock().unwrap();
        tasks.get(&task_id).and_then(|t| t.result.clone())
    }
}
