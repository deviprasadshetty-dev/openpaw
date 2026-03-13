use crate::bus::Bus;
use crate::config::Config;
use crate::session::SessionManager;
use crate::agent::Agent;
use crate::daemon::{create_provider, build_tools};
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
    pub last_heartbeat: u64,
    pub iteration_count: u32,
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
    daemon_config: Config,
    bus: Arc<Bus>,
    session_manager: Arc<Mutex<Option<Arc<SessionManager>>>>,
}

impl SubagentManager {
    pub fn new(bus: Arc<Bus>, subagent_config: SubagentConfig, daemon_config: Config) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            config: subagent_config,
            daemon_config,
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
        self.spawn_with_agent(task, label, origin_channel, origin_chat_id, None)
    }

    pub fn spawn_with_agent(
        &self,
        task: &str,
        label: &str,
        origin_channel: &str,
        origin_chat_id: &str,
        agent_name: Option<&str>,
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
            last_heartbeat: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            iteration_count: 0,
        };

        tasks.insert(task_id, state);

        let task_copy = task.to_string();
        let label_copy = label.to_string();
        let origin_channel_copy = origin_channel.to_string();
        let origin_chat_copy = origin_chat_id.to_string();
        let agent_name_copy = agent_name.map(|s| s.to_string());

        let tasks_clone = self.tasks.clone();
        let bus_clone = self.bus.clone();
        
        // Build subagent configuration based on daemon config + possible agent override
        let mut sub_config = self.daemon_config.clone();
        let mut custom_system_prompt = None;
        if let Some(ref name) = agent_name_copy {
            if let Some(agent_cfg) = sub_config.agents.iter().find(|a| &a.name == name) {
                sub_config.default_provider = agent_cfg.provider.clone();
                sub_config.default_model = Some(agent_cfg.model.clone());
                if let Some(t) = agent_cfg.temperature {
                    sub_config.default_temperature = Some(t as f32);
                }
                custom_system_prompt = agent_cfg.system_prompt.clone();
            } else {
                tasks.remove(&task_id);
                return Err(anyhow!("Unknown agent profile: {}", name));
            }
        }

        let max_iterations = self.config.max_iterations;

        // Spawn asynchronous task
        tokio::spawn(async move {
            let context = crate::tools::root::ToolContext {
                channel: origin_channel_copy.clone(),
                sender_id: "subagent".to_string(),
                chat_id: origin_chat_copy.clone(),
                session_key: subagent_session_key.clone(),
            };

            // Instantiate isolated tools and provider
            let provider = create_provider(&sub_config);
            let subagent_tools = build_tools(&sub_config, None, None, None, None).await; // None avoids adding 'spawn' tool again

            let mut agent = Agent::new(
                provider,
                subagent_tools,
                sub_config.default_model.clone().unwrap_or("gpt-4o".to_string()),
                sub_config.workspace_dir.clone(),
            );
            agent.max_tool_iterations = max_iterations;
            
            if let Some(prompt) = custom_system_prompt {
                // If a custom prompt is provided, we inject it directly
                agent.has_system_prompt = true;
                let mut content = format!("{}\n\nYou are running as a background subagent.\n\n", prompt);
                crate::agent::prompt::append_date_time_section(&mut content);
                agent.history.push(crate::providers::ChatMessage {
                    role: "system".to_string(),
                    thought_signature: None,
                    content,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    content_parts: None,
                });
            } else {
                // BUG-7 FIX: The old 1-line prompt was too vague. A subagent gets
                // exactly one turn() call, so it must complete the ENTIRE task —
                // including all tool calls, reasoning steps, and final synthesis —
                // within that single turn. The prompt now makes this explicit.
                agent.has_system_prompt = true;
                let mut subagent_prompt = concat!(
                    "You are an autonomous background subagent. ",
                    "Your job is to FULLY complete the assigned task in a single response ",
                    "using whatever tools are available to you.\n\n",
                    "## Critical Rules\n",
                    "1. **Never stop mid-task.** Keep calling tools until the work is 100% done.\n",
                    "2. **Do not promise future action.** You will not get another turn — if you cannot ",
                    "complete something, explain why clearly in your final response.\n",
                    "3. **Use tools proactively.** Read files, run commands, search the web — ",
                    "do not guess when you can verify with a tool.\n",
                    "4. **Always finish with a clear summary** of what you did, what succeeded, ",
                    "and what (if anything) could not be completed and why.\n",
                    "5. **Be concise in your final output.** The person who assigned this task wants results, not narration.\n\n"
                ).to_string();
                crate::agent::prompt::append_date_time_section(&mut subagent_prompt);
                agent.history.push(crate::providers::ChatMessage {
                    role: "system".to_string(),
                    content: subagent_prompt,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    content_parts: None,
                    thought_signature: None,
                });
            }

            let result = agent.turn(task_copy, &context).await;

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

    pub fn monitor_tick(&self) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        for (id, state) in tasks.iter_mut() {
            if state.status == TaskStatus::Running {
                // Check for timeout (1 hour)
                if now > state.started_at + 3600 {
                    state.status = TaskStatus::Failed;
                    state.error_msg = Some("Task timed out (exceeded 1 hour)".to_string());
                    state.completed_at = Some(now);

                    let report = format!(
                        "⚠️ Subagent watchdog: Task '{}' (ID {}) has timed out after 1 hour and was terminated.",
                        state.label, id
                    );
                    
                    // We can't easily get the origin from TaskState yet, but we can log it
                    // and publish to a general system channel if we had one.
                    // For now, let's just log it and rely on the user checking status.
                    tracing::warn!("{}", report);
                }
            }
        }
    }
}
