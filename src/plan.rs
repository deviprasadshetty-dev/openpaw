/// Dependency-aware task planning and execution.
///
/// The main agent creates a `Plan` (a DAG of subtasks with `depends_on` references),
/// and the `PlanManager` executes it using the `SubagentManager` in topological order —
/// running independent tasks concurrently within each dependency level.
use crate::bus::{Bus, make_outbound};
use crate::skillmint::{ImportanceLevel, MintResult, SkillMint};
use crate::subagent::{SubagentManager, TaskStatus};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanTask {
    /// Unique identifier within the plan (e.g. "research", "draft", "review").
    pub id: String,
    /// Human-readable description / prompt for the subagent.
    pub description: String,
    /// IDs of tasks that must complete before this one starts.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Optional named agent profile to use (from config.agents[].name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlanStatus {
    Running,
    Completed,
    PartialFailure,
    Failed,
}

#[derive(Debug, Clone)]
pub struct PlanExecution {
    pub plan_id: u64,
    pub goal: String,
    pub status: PlanStatus,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    /// Maps plan task id -> subagent task_id
    pub task_map: HashMap<String, u64>,
    /// Maps plan task id -> final status
    pub task_outcomes: HashMap<String, TaskStatus>,
}

pub struct PlanManager {
    plans: Arc<Mutex<HashMap<u64, PlanExecution>>>,
    next_id: Arc<Mutex<u64>>,
    subagent_manager: Arc<SubagentManager>,
    bus: Arc<Bus>,
    /// Optional SkillMint instance. When present, auto-minting runs after
    /// each fully-successful plan completion.
    skillmint: Option<Arc<SkillMint>>,
    /// Resolved distillation model (from resolve_distill_model at startup).
    distill_model: String,
}

impl PlanManager {
    pub fn new(subagent_manager: Arc<SubagentManager>, bus: Arc<Bus>) -> Self {
        Self {
            plans: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            subagent_manager,
            bus,
            skillmint: None,
            distill_model: String::new(),
        }
    }

    /// Attach a SkillMint instance so the plan manager can auto-mint after completions.
    pub fn with_skillmint(mut self, sm: Arc<SkillMint>, distill_model: String) -> Self {
        self.skillmint = Some(sm);
        self.distill_model = distill_model;
        self
    }

    /// Validate the task list and return topologically sorted batches.
    /// Each batch is a set of task IDs whose dependencies are all in earlier batches.
    fn topo_sort(tasks: &[PlanTask]) -> Result<Vec<Vec<String>>> {
        // Validate all dependency references
        let task_ids: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        for task in tasks {
            for dep in &task.depends_on {
                if !task_ids.contains(dep.as_str()) {
                    return Err(anyhow!(
                        "Task '{}' depends on unknown task '{}'",
                        task.id,
                        dep
                    ));
                }
            }
        }

        // Kahn's algorithm
        let mut in_degree: HashMap<&str, usize> = tasks.iter().map(|t| (t.id.as_str(), 0)).collect();
        let mut dependents: HashMap<&str, Vec<&str>> = tasks.iter().map(|t| (t.id.as_str(), vec![])).collect();

        for task in tasks {
            for dep in &task.depends_on {
                *in_degree.get_mut(task.id.as_str()).unwrap() += 1;
                dependents.get_mut(dep.as_str()).unwrap().push(task.id.as_str());
            }
        }

        let mut batches: Vec<Vec<String>> = Vec::new();
        let mut queue: Vec<&str> = in_degree
            .iter()
            .filter(|&(_, deg)| *deg == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut processed = 0usize;

        while !queue.is_empty() {
            batches.push(queue.iter().map(|s| s.to_string()).collect());
            let mut next_queue = Vec::new();
            for id in &queue {
                processed += 1;
                for &dep in &dependents[id] {
                    let deg = in_degree.get_mut(dep).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        next_queue.push(dep);
                    }
                }
            }
            queue = next_queue;
        }

        if processed != tasks.len() {
            return Err(anyhow!(
                "Cycle detected in task dependencies — cannot execute plan"
            ));
        }

        Ok(batches)
    }

    /// Submit a plan for execution. Returns the plan_id immediately; execution runs
    /// in a background tokio task, reporting each batch's progress to the origin channel.
    pub fn execute_plan(
        &self,
        goal: &str,
        tasks: Vec<PlanTask>,
        origin_channel: &str,
        origin_chat_id: &str,
    ) -> Result<u64> {
        let batches = Self::topo_sort(&tasks)?;

        let mut id_guard = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
        let plan_id = *id_guard;
        *id_guard += 1;
        drop(id_guard);

        let task_lookup: HashMap<String, PlanTask> = tasks
            .into_iter()
            .map(|t| (t.id.clone(), t))
            .collect();

        let exec = PlanExecution {
            plan_id,
            goal: goal.to_string(),
            status: PlanStatus::Running,
            started_at: now_secs(),
            completed_at: None,
            task_map: HashMap::new(),
            task_outcomes: HashMap::new(),
        };

        self.plans
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(plan_id, exec);

        let plans_clone = self.plans.clone();
        let sm = self.subagent_manager.clone();
        let bus = self.bus.clone();
        let origin_channel = origin_channel.to_string();
        let origin_chat_id = origin_chat_id.to_string();
        let goal_str = goal.to_string();
        let skillmint_opt = self.skillmint.clone();
        let distill_model = self.distill_model.clone();

        tokio::spawn(async move {
            let total_batches = batches.len();
            let total_tasks: usize = batches.iter().map(|b| b.len()).sum();

            let start_msg = format!(
                "📋 Plan {} started: \"{}\"\n{} tasks in {} batch(es)",
                plan_id, goal_str, total_tasks, total_batches
            );
            let _ = bus.publish_outbound(make_outbound(&origin_channel, &origin_chat_id, &start_msg));

            let mut any_failed = false;

            'outer: for (batch_idx, batch) in batches.iter().enumerate() {
                info!("Plan {}: executing batch {}/{}", plan_id, batch_idx + 1, total_batches);

                let batch_msg = format!(
                    "📋 Plan {} — batch {}/{}: starting {}",
                    plan_id,
                    batch_idx + 1,
                    total_batches,
                    batch.join(", ")
                );
                let _ = bus.publish_outbound(make_outbound(&origin_channel, &origin_chat_id, &batch_msg));

                // Spawn all tasks in this batch
                let mut batch_spawn: Vec<(String, u64)> = Vec::new();
                for task_id in batch {
                    let task = match task_lookup.get(task_id) {
                        Some(t) => t,
                        None => {
                            warn!("Plan {}: task '{}' not found in lookup", plan_id, task_id);
                            continue;
                        }
                    };

                    let label = format!("plan-{}-{}", plan_id, task.id);
                    match sm.spawn_with_agent(
                        &task.description,
                        &label,
                        &origin_channel,
                        &origin_chat_id,
                        task.agent_id.as_deref(),
                    ) {
                        Ok(sub_id) => {
                            // Record the mapping
                            if let Ok(mut guard) = plans_clone.lock() {
                                if let Some(plan) = guard.get_mut(&plan_id) {
                                    plan.task_map.insert(task_id.clone(), sub_id);
                                }
                            }
                            batch_spawn.push((task_id.clone(), sub_id));
                        }
                        Err(e) => {
                            warn!("Plan {}: failed to spawn task '{}': {}", plan_id, task_id, e);
                            any_failed = true;
                        }
                    }
                }

                // Poll until all spawned tasks in this batch are done
                loop {
                    tokio::time::sleep(Duration::from_secs(3)).await;

                    let all_done = batch_spawn.iter().all(|(_, sub_id)| {
                        matches!(
                            sm.get_task_status(*sub_id),
                            Some(
                                TaskStatus::Completed
                                    | TaskStatus::Failed
                                    | TaskStatus::Cancelled
                            )
                        )
                    });

                    if all_done {
                        // Record outcomes
                        for (task_id, sub_id) in &batch_spawn {
                            let outcome = sm
                                .get_task_status(*sub_id)
                                .unwrap_or(TaskStatus::Failed);
                            if outcome == TaskStatus::Failed || outcome == TaskStatus::Cancelled {
                                any_failed = true;
                            }
                            if let Ok(mut guard) = plans_clone.lock() {
                                if let Some(plan) = guard.get_mut(&plan_id) {
                                    plan.task_outcomes.insert(task_id.clone(), outcome);
                                }
                            }
                        }
                        break;
                    }

                    // Timeout guard: abort plan if it's been running for more than 2 hours
                    let started = {
                        let guard = plans_clone.lock().unwrap_or_else(|e| e.into_inner());
                        guard.get(&plan_id).map(|p| p.started_at).unwrap_or(0)
                    };
                    if now_secs().saturating_sub(started) > 7200 {
                        warn!("Plan {} timed out", plan_id);
                        any_failed = true;
                        break 'outer;
                    }
                }
            }

            // Finalize plan
            let final_status = if any_failed {
                PlanStatus::PartialFailure
            } else {
                PlanStatus::Completed
            };

            let final_msg = if final_status == PlanStatus::Completed {
                format!("✅ Plan {} completed: \"{}\"", plan_id, goal_str)
            } else {
                format!(
                    "⚠️ Plan {} finished with some failures: \"{}\"",
                    plan_id, goal_str
                )
            };
            let _ = bus.publish_outbound(make_outbound(&origin_channel, &origin_chat_id, &final_msg));

            if let Ok(mut guard) = plans_clone.lock() {
                if let Some(plan) = guard.get_mut(&plan_id) {
                    plan.status = final_status.clone();
                    plan.completed_at = Some(now_secs());
                }
            }

            // --- SkillMint auto-mint hook ---
            // Capture metrics before the async spawn moves them away.
            let duration_secs = {
                let guard = plans_clone.lock().unwrap_or_else(|e| e.into_inner());
                let started = guard.get(&plan_id).map(|p| p.started_at).unwrap_or(0);
                now_secs().saturating_sub(started)
            };
            let task_outcomes_snapshot: HashMap<String, TaskStatus> = {
                let guard = plans_clone.lock().unwrap_or_else(|e| e.into_inner());
                guard
                    .get(&plan_id)
                    .map(|p| p.task_outcomes.clone())
                    .unwrap_or_default()
            };

            if final_status == PlanStatus::Completed {
                if let Some(ref sm_arc) = skillmint_opt {
                    let summary =
                        build_plan_summary(&goal_str, total_tasks, &task_outcomes_snapshot);
                    let sm_ref = sm_arc.clone();
                    let model = distill_model.clone();
                    let goal_clone = goal_str.clone();

                    tokio::spawn(async move {
                        let result = sm_ref
                            .maybe_mint(
                                &goal_clone,
                                &summary,
                                total_tasks,
                                true,
                                duration_secs,
                                &model,
                                // Provider wired in Phase 6 follow-up
                                |_m, _p| async { Ok(String::new()) },
                            )
                            .await;

                        match result {
                            MintResult::Minted { slug, importance, is_upgrade } => {
                                let verb = if is_upgrade { "upgraded" } else { "minted" };
                                match importance {
                                    ImportanceLevel::Normal => {
                                        info!("SkillMint: {} skill '{}'", verb, slug);
                                    }
                                    ImportanceLevel::High => {
                                        info!(
                                            "SkillMint: {} high-importance skill '{}' — notifying user",
                                            verb, slug
                                        );
                                        // TODO: publish approval-request to origin_channel
                                    }
                                }
                            }
                            MintResult::Skipped { reason } => {
                                info!("SkillMint: skipped — {}", reason);
                            }
                        }
                    });
                }
            }
        });

        Ok(plan_id)
    }

    /// Get a human-readable status summary for a plan.
    pub fn get_status(&self, plan_id: u64) -> Option<String> {
        let guard = self.plans.lock().unwrap_or_else(|e| e.into_inner());
        let plan = guard.get(&plan_id)?;

        let status_str = match plan.status {
            PlanStatus::Running => "running",
            PlanStatus::Completed => "completed",
            PlanStatus::PartialFailure => "partial failure",
            PlanStatus::Failed => "failed",
        };

        let outcomes: Vec<String> = plan
            .task_outcomes
            .iter()
            .map(|(id, s)| format!("{}: {:?}", id, s))
            .collect();

        Some(format!(
            "Plan {} — \"{}\"\nStatus: {}\nTasks: {}",
            plan_id,
            plan.goal,
            status_str,
            if outcomes.is_empty() {
                "(in progress)".to_string()
            } else {
                outcomes.join(", ")
            }
        ))
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Build a concise plain-text summary of what the plan did.
/// Used as the `plan_summary` argument to SkillMint distillation.
fn build_plan_summary(
    goal: &str,
    total_tasks: usize,
    outcomes: &HashMap<String, TaskStatus>,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Goal: {}", goal));
    lines.push(format!("Total tasks: {}", total_tasks));
    lines.push("Tasks:".to_string());

    let mut task_ids: Vec<&String> = outcomes.keys().collect();
    task_ids.sort();
    for id in task_ids {
        let status = match outcomes.get(id) {
            Some(TaskStatus::Completed) => "✅ completed",
            Some(TaskStatus::Failed) => "❌ failed",
            Some(TaskStatus::Cancelled) => "⚠️ cancelled",
            _ => "❓ unknown",
        };
        lines.push(format!("  - {}: {}", id, status));
    }

    lines.join("\n")
}
