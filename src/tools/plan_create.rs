/// Feature 2: Structured task planning with dependency-aware execution.
///
/// Create a plan: a DAG of named subtasks with `depends_on` references.
/// The PlanManager executes tasks in topological order, running independent
/// tasks concurrently using the SubagentManager.
use super::{Tool, ToolContext, ToolResult};
use crate::plan::{PlanManager, PlanTask};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct PlanCreateTool {
    pub plan_manager: Arc<PlanManager>,
}

#[async_trait]
impl Tool for PlanCreateTool {
    fn name(&self) -> &str {
        "plan_create"
    }

    fn description(&self) -> &str {
        "Decompose a goal into ordered subtasks with dependencies, then execute them \
         using background subagents. Independent tasks run concurrently; dependent tasks \
         wait for their dependencies to complete first. \
         Returns a plan_id immediately — execution proceeds in the background with \
         progress updates sent to this channel. \
         Use plan_status to check progress."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "High-level description of what the plan achieves"
                },
                "tasks": {
                    "type": "array",
                    "description": "List of subtasks to execute",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Unique identifier for this task within the plan (e.g. 'research', 'draft', 'review')"
                            },
                            "description": {
                                "type": "string",
                                "description": "Full task description / prompt for the subagent"
                            },
                            "depends_on": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "IDs of tasks that must complete before this one starts (empty = no dependencies)"
                            },
                            "agent_id": {
                                "type": "string",
                                "description": "Optional named agent profile to use for this task (from config.agents)"
                            }
                        },
                        "required": ["id", "description"]
                    },
                    "minItems": 1
                },
                "plan_id": {
                    "type": "integer",
                    "description": "If provided, return the status of an existing plan instead of creating a new one"
                }
            }
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolResult> {
        // Status check mode
        if let Some(plan_id) = args.get("plan_id").and_then(|v| v.as_u64()) {
            return match self.plan_manager.get_status(plan_id) {
                Some(status) => Ok(ToolResult::ok(status)),
                None => Ok(ToolResult::fail(format!("Plan {} not found.", plan_id))),
            };
        }

        let goal = match args.get("goal").and_then(|v| v.as_str()) {
            Some(g) if !g.trim().is_empty() => g.trim().to_string(),
            _ => return Ok(ToolResult::fail("Missing or empty 'goal' parameter")),
        };

        let tasks_val = match args.get("tasks").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr,
            _ => return Ok(ToolResult::fail("'tasks' must be a non-empty array")),
        };

        let mut tasks = Vec::new();
        for (i, t) in tasks_val.iter().enumerate() {
            let id = match t.get("id").and_then(|v| v.as_str()) {
                Some(s) if !s.trim().is_empty() => s.trim().to_string(),
                _ => return Ok(ToolResult::fail(format!("Task[{}] missing 'id'", i))),
            };
            let description = match t.get("description").and_then(|v| v.as_str()) {
                Some(s) if !s.trim().is_empty() => s.trim().to_string(),
                _ => {
                    return Ok(ToolResult::fail(format!(
                        "Task '{}' missing 'description'",
                        id
                    )));
                }
            };
            let depends_on: Vec<String> = t
                .get("depends_on")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            let agent_id = t
                .get("agent_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            tasks.push(PlanTask {
                id,
                description,
                depends_on,
                agent_id,
            });
        }

        match self
            .plan_manager
            .execute_plan(&goal, tasks, &context.channel, &context.chat_id)
        {
            Ok(plan_id) => Ok(ToolResult::ok(format!(
                "📋 Plan {} created and started for: \"{}\"\n\
                 Tasks are executing in the background. Progress updates will appear in this channel.\n\
                 Use plan_create with plan_id={} to check status.",
                plan_id, goal, plan_id
            ))),
            Err(e) => Ok(ToolResult::fail(format!("Failed to create plan: {}", e))),
        }
    }
}
