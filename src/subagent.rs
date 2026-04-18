use crate::agent_mailbox::AgentMailbox;
use crate::approval::ApprovalManager;
use crate::bus::Bus;
use crate::config::Config;
use crate::session::SessionManager;
use crate::agent::Agent;
use crate::daemon::{create_provider, build_tools};
use anyhow::{Result, anyhow};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
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
    pub origin_channel: String,
    pub origin_chat_id: String,
    pub abort_handle: Option<tokio::task::AbortHandle>,
}

struct PendingTask {
    task: String,
    label: String,
    origin_channel: String,
    origin_chat_id: String,
    agent_name: Option<String>,
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
    pending: Arc<Mutex<VecDeque<PendingTask>>>,
    /// Shared mailbox for inter-agent messaging (Feature 3)
    pub mailbox: Arc<AgentMailbox>,
    /// Shared approval manager for human-in-the-loop (Feature 5)
    pub approval_manager: Arc<ApprovalManager>,
}

impl SubagentManager {
    pub fn new(
        bus: Arc<Bus>,
        subagent_config: SubagentConfig,
        daemon_config: Config,
        mailbox: Arc<AgentMailbox>,
        approval_manager: Arc<ApprovalManager>,
    ) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            config: subagent_config,
            daemon_config,
            bus,
            session_manager: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(VecDeque::new())),
            mailbox,
            approval_manager,
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

        let mut next_id = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
        let task_id = *next_id;
        *next_id += 1;

        // If at capacity, queue the task rather than erroring
        if running_count >= self.config.max_concurrent as usize {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending.push_back(PendingTask {
                task: task.to_string(),
                label: label.to_string(),
                origin_channel: origin_channel.to_string(),
                origin_chat_id: origin_chat_id.to_string(),
                agent_name: agent_name.map(|s| s.to_string()),
            });

            let state = TaskState {
                status: TaskStatus::Queued,
                label: label.to_string(),
                session_key: None,
                result: None,
                error_msg: None,
                started_at: now_secs(),
                completed_at: None,
                last_heartbeat: now_secs(),
                iteration_count: 0,
                origin_channel: origin_channel.to_string(),
                origin_chat_id: origin_chat_id.to_string(),
                abort_handle: None,
            };
            tasks.insert(task_id, state);

            let msg = format!(
                "⏳ Subagent '{}' (Task ID: {}) queued — {} slots in use, will start when a slot frees up.",
                label, task_id, running_count
            );
            let outbound = crate::bus::make_outbound(origin_channel, origin_chat_id, &msg);
            let _ = self.bus.publish_outbound(outbound);

            return Ok(task_id);
        }

        self.launch_task_locked(
            &mut tasks,
            task_id,
            task,
            label,
            origin_channel,
            origin_chat_id,
            agent_name,
        )
    }

    /// Internal: launches a task, inserting into `tasks`. Must hold `tasks` lock.
    fn launch_task_locked(
        &self,
        tasks: &mut HashMap<u64, TaskState>,
        task_id: u64,
        task: &str,
        label: &str,
        origin_channel: &str,
        origin_chat_id: &str,
        agent_name: Option<&str>,
    ) -> Result<u64> {
        let subagent_session_key = format!("subagent:{}", task_id);

        let state = TaskState {
            status: TaskStatus::Running,
            label: label.to_string(),
            session_key: Some(subagent_session_key.clone()),
            result: None,
            error_msg: None,
            started_at: now_secs(),
            completed_at: None,
            last_heartbeat: now_secs(),
            iteration_count: 0,
            origin_channel: origin_channel.to_string(),
            origin_chat_id: origin_chat_id.to_string(),
            abort_handle: None,
        };
        tasks.insert(task_id, state);

        let task_copy = task.to_string();
        let label_copy = label.to_string();
        let origin_channel_copy = origin_channel.to_string();
        let origin_chat_copy = origin_chat_id.to_string();
        let agent_name_copy = agent_name.map(|s| s.to_string());

        let tasks_clone = self.tasks.clone();
        let bus_clone = self.bus.clone();
        let pending_clone = self.pending.clone();
        let daemon_config_clone = self.daemon_config.clone();
        let max_iterations = self.config.max_iterations;
        let max_concurrent = self.config.max_concurrent;
        let mailbox_clone = self.mailbox.clone();
        let approval_clone = self.approval_manager.clone();

        // Build subagent config (may override provider/model if a named agent profile is given)
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

        let join_handle = tokio::spawn(async move {
            let context = crate::tools::root::ToolContext {
                channel: origin_channel_copy.clone(),
                sender_id: "subagent".to_string(),
                chat_id: origin_chat_copy.clone(),
                session_key: subagent_session_key.clone(),
                task_kind: Some("subagent".to_string()),
            };

            let provider = create_provider(&sub_config);
            // Pass None for subagent_manager to prevent recursive spawning;
            // all other capabilities (memory, cron, goals) are intentionally
            // excluded to keep subagents lightweight and isolated.
            // Subagents DO get: bus (for task_progress), mailbox, and approval_manager.
            let subagent_tools = build_tools(
                &sub_config,
                None,
                None,
                None,
                None,
                Some(bus_clone.clone()),
                Some(mailbox_clone.clone()),
                Some(approval_clone.clone()),
                None, // event_registry — main agent only
                None, // plan_manager — main agent only
            )
            .await;

            let sub_model = sub_config.default_model.clone().unwrap_or("gpt-4o".to_string());
            let mut agent = Agent::new(
                provider,
                subagent_tools,
                sub_model.clone(),
                sub_config.workspace_dir.clone(),
            );

            // Apply task-based model routing from config
            let task_config = crate::model_router::TaskModelConfig::with_overrides(
                &sub_model,
                &sub_config.task_models.to_map(),
            );
            agent = agent.with_task_models(&task_config);
            agent.max_tool_iterations = max_iterations;

            if let Some(prompt) = custom_system_prompt {
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
                // Don't overwrite a Cancelled state set by cancel_task()
                if state.status == TaskStatus::Cancelled {
                    return;
                }
                match result {
                    Ok(response) => {
                        state.status = TaskStatus::Completed;
                        state.result = Some(response.clone());
                        state.completed_at = Some(now_secs());

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
                        state.completed_at = Some(now_secs());

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
            drop(tasks);

            // Drain the pending queue now that a slot has opened
            drain_pending(tasks_clone, pending_clone, bus_clone, daemon_config_clone, max_iterations, max_concurrent, mailbox_clone, approval_clone);
        });

        // Store the abort handle so we can cancel the task later
        let abort_handle = join_handle.abort_handle();
        if let Some(state) = tasks.get_mut(&task_id) {
            state.abort_handle = Some(abort_handle);
        }
        // Drop the JoinHandle — the task keeps running detached
        drop(join_handle);

        Ok(task_id)
    }

    pub fn cancel_task(&self, task_id: u64) -> Result<()> {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        match tasks.get_mut(&task_id) {
            None => Err(anyhow!("Task {} not found", task_id)),
            Some(state) => match state.status {
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled => {
                    Err(anyhow!("Task {} is already {:?}", task_id, state.status))
                }
                TaskStatus::Queued => {
                    // Remove from pending queue
                    let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
                    pending.retain(|p| {
                        // Match by label + channel as a best-effort proxy (task_id not stored in PendingTask)
                        !(p.label == state.label && p.origin_channel == state.origin_channel)
                    });
                    state.status = TaskStatus::Cancelled;
                    state.completed_at = Some(now_secs());
                    Ok(())
                }
                TaskStatus::Running => {
                    if let Some(handle) = state.abort_handle.take() {
                        handle.abort();
                    }
                    state.status = TaskStatus::Cancelled;
                    state.completed_at = Some(now_secs());

                    let report = format!(
                        "🚫 Subagent '{}' (ID {}) was cancelled.",
                        state.label, task_id
                    );
                    let outbound = crate::bus::make_outbound(
                        &state.origin_channel,
                        &state.origin_chat_id,
                        &report,
                    );
                    let _ = self.bus.publish_outbound(outbound);
                    Ok(())
                }
            },
        }
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
        let now = now_secs();

        for (id, state) in tasks.iter_mut() {
            if state.status == TaskStatus::Running && now > state.started_at + 3600 {
                if let Some(handle) = state.abort_handle.take() {
                    handle.abort();
                }
                state.status = TaskStatus::Failed;
                state.error_msg = Some("Task timed out (exceeded 1 hour)".to_string());
                state.completed_at = Some(now);

                let report = format!(
                    "⚠️ Subagent '{}' (ID {}) timed out after 1 hour and was terminated.",
                    state.label, id
                );
                tracing::warn!("{}", report);

                // Now we can report back to the origin channel
                let outbound = crate::bus::make_outbound(
                    &state.origin_channel,
                    &state.origin_chat_id,
                    &report,
                );
                let _ = self.bus.publish_outbound(outbound);
            }
        }
        drop(tasks);

        // Drain pending queue in case monitor_tick fires after a task finished
        // without the completion callback running (e.g., process restart edge cases)
        let tasks_clone = self.tasks.clone();
        let pending_clone = self.pending.clone();
        let bus_clone = self.bus.clone();
        let daemon_config_clone = self.daemon_config.clone();
        let max_iterations = self.config.max_iterations;
        let max_concurrent = self.config.max_concurrent;
        let mailbox_clone = self.mailbox.clone();
        let approval_clone = self.approval_manager.clone();
        drain_pending(tasks_clone, pending_clone, bus_clone, daemon_config_clone, max_iterations, max_concurrent, mailbox_clone, approval_clone);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Promote pending tasks into running slots as capacity permits.
fn drain_pending(
    tasks: Arc<Mutex<HashMap<u64, TaskState>>>,
    pending: Arc<Mutex<VecDeque<PendingTask>>>,
    bus: Arc<Bus>,
    daemon_config: Config,
    max_iterations: u32,
    max_concurrent: u32,
    mailbox: Arc<AgentMailbox>,
    approval_manager: Arc<ApprovalManager>,
) {
    loop {
        let next = {
            let tasks_guard = tasks.lock().unwrap_or_else(|e| e.into_inner());
            let running = tasks_guard
                .values()
                .filter(|t| t.status == TaskStatus::Running)
                .count();
            if running >= max_concurrent as usize {
                break;
            }
            let mut pending_guard = pending.lock().unwrap_or_else(|e| e.into_inner());
            pending_guard.pop_front()
        };

        match next {
            None => break,
            Some(pt) => {
                // Find the Queued task_id for this pending entry (match by label + channel)
                let task_id = {
                    let tasks_guard = tasks.lock().unwrap_or_else(|e| e.into_inner());
                    tasks_guard.iter()
                        .find(|(_, s)| {
                            s.status == TaskStatus::Queued
                                && s.label == pt.label
                                && s.origin_channel == pt.origin_channel
                        })
                        .map(|(id, _)| *id)
                };

                let task_id = match task_id {
                    Some(id) => id,
                    None => continue, // was cancelled
                };

                let task_copy = pt.task.clone();
                let label_copy = pt.label.clone();
                let origin_channel_copy = pt.origin_channel.clone();
                let origin_chat_copy = pt.origin_chat_id.clone();
                let agent_name_copy = pt.agent_name.clone();

                let mut sub_config = daemon_config.clone();
                let mut custom_system_prompt = None;
                if let Some(ref name) = agent_name_copy {
                    if let Some(agent_cfg) = sub_config.agents.iter().find(|a| &a.name == name) {
                        sub_config.default_provider = agent_cfg.provider.clone();
                        sub_config.default_model = Some(agent_cfg.model.clone());
                        if let Some(t) = agent_cfg.temperature {
                            sub_config.default_temperature = Some(t as f32);
                        }
                        custom_system_prompt = agent_cfg.system_prompt.clone();
                    }
                }

                let subagent_session_key = format!("subagent:{}", task_id);

                // Transition from Queued → Running
                {
                    let mut tasks_guard = tasks.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(state) = tasks_guard.get_mut(&task_id) {
                        state.status = TaskStatus::Running;
                        state.session_key = Some(subagent_session_key.clone());
                        state.started_at = now_secs();
                        state.last_heartbeat = now_secs();
                    }
                }

                let tasks_clone2 = tasks.clone();
                let pending_clone2 = pending.clone();
                let bus_clone2 = bus.clone();
                let daemon_config_clone2 = daemon_config.clone();
                let mailbox_clone2 = mailbox.clone();
                let approval_clone2 = approval_manager.clone();

                let join_handle = tokio::spawn(async move {
                    let context = crate::tools::root::ToolContext {
                        channel: origin_channel_copy.clone(),
                        sender_id: "subagent".to_string(),
                        chat_id: origin_chat_copy.clone(),
                        session_key: subagent_session_key.clone(),
                        task_kind: Some("subagent".to_string()),
                    };

                    let provider = create_provider(&sub_config);
                    let subagent_tools = build_tools(
                        &sub_config,
                        None,
                        None,
                        None,
                        None,
                        Some(bus_clone2.clone()),
                        Some(mailbox_clone2.clone()),
                        Some(approval_clone2.clone()),
                        None,
                        None,
                    )
                    .await;

                    let sub_model2 = sub_config.default_model.clone().unwrap_or("gpt-4o".to_string());
                    let mut agent = Agent::new(
                        provider,
                        subagent_tools,
                        sub_model2.clone(),
                        sub_config.workspace_dir.clone(),
                    );

                    // Apply task-based model routing from config
                    let task_config = crate::model_router::TaskModelConfig::with_overrides(
                        &sub_model2,
                        &sub_config.task_models.to_map(),
                    );
                    agent = agent.with_task_models(&task_config);
                    agent.max_tool_iterations = max_iterations;

                    if let Some(prompt) = custom_system_prompt {
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
                        agent.has_system_prompt = true;
                        let mut subagent_prompt = concat!(
                            "You are an autonomous background subagent. ",
                            "Your job is to FULLY complete the assigned task in a single response ",
                            "using whatever tools are available to you.\n\n",
                            "## Critical Rules\n",
                            "1. **Never stop mid-task.** Keep calling tools until the work is 100% done.\n",
                            "2. **Do not promise future action.** You will not get another turn.\n",
                            "3. **Use tools proactively.** Read files, run commands, search the web.\n",
                            "4. **Always finish with a clear summary** of what you did.\n",
                            "5. **Be concise in your final output.**\n\n"
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

                    let mut tasks = tasks_clone2.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(state) = tasks.get_mut(&task_id) {
                        if state.status == TaskStatus::Cancelled {
                            return;
                        }
                        match result {
                            Ok(response) => {
                                state.status = TaskStatus::Completed;
                                state.result = Some(response.clone());
                                state.completed_at = Some(now_secs());
                                let report = format!(
                                    "✅ Subagent '{}' (ID {}) completed:\n\n{}",
                                    label_copy, task_id, response
                                );
                                let outbound = crate::bus::make_outbound(
                                    &origin_channel_copy,
                                    &origin_chat_copy,
                                    &report,
                                );
                                let _ = bus_clone2.publish_outbound(outbound);
                            }
                            Err(e) => {
                                state.status = TaskStatus::Failed;
                                state.error_msg = Some(e.to_string());
                                state.completed_at = Some(now_secs());
                                let report = format!(
                                    "❌ Subagent '{}' (ID {}) failed: {}",
                                    label_copy, task_id, e
                                );
                                let outbound = crate::bus::make_outbound(
                                    &origin_channel_copy,
                                    &origin_chat_copy,
                                    &report,
                                );
                                let _ = bus_clone2.publish_outbound(outbound);
                            }
                        }
                    }
                    drop(tasks);
                    drain_pending(tasks_clone2, pending_clone2, bus_clone2, daemon_config_clone2, max_iterations, max_concurrent, mailbox_clone2, approval_clone2);
                });

                let abort_handle = join_handle.abort_handle();
                drop(join_handle);

                let mut tasks_guard = tasks.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(state) = tasks_guard.get_mut(&task_id) {
                    state.abort_handle = Some(abort_handle);
                }
            }
        }
    }
}
