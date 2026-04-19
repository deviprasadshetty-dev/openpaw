use crate::goals::{GoalManager, GoalStatus};
use crate::tools::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct GoalAddTool {
    pub manager: Arc<GoalManager>,
}

#[async_trait]
impl Tool for GoalAddTool {
    fn name(&self) -> &str {
        "goal_add"
    }

    fn description(&self) -> &str {
        "Add a new long-term goal or project objective."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "The description of the goal."
                },
                "priority": {
                    "type": "integer",
                    "description": "Priority from 1 (highest) to 5 (lowest). Defaults to 3.",
                    "default": 3
                }
            },
            "required": ["description"]
        }"#
        .to_string()
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let description = arguments["description"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing description"))?;
        let priority = arguments["priority"].as_u64().unwrap_or(3) as u8;

        let id = self.manager.add_goal(description, priority);
        Ok(ToolResult::ok(format!(
            "Goal added successfully with ID: {}",
            id
        )))
    }
}

pub struct GoalListTool {
    pub manager: Arc<GoalManager>,
}

#[async_trait]
impl Tool for GoalListTool {
    fn name(&self) -> &str {
        "goal_list"
    }

    fn description(&self) -> &str {
        "List all current long-term goals and objectives."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {}
        }"#
        .to_string()
    }

    async fn execute(&self, _arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let goals = self.manager.list_goals();
        if goals.is_empty() {
            return Ok(ToolResult::ok("No goals found."));
        }

        let mut output = String::from("Current Goals:\n");
        for goal in goals {
            output.push_str(&format!(
                "- [{}] (ID: {}) {} (Status: {:?}, Priority: {})\n",
                if matches!(goal.status, GoalStatus::Completed) {
                    "x"
                } else {
                    " "
                },
                goal.id,
                goal.description,
                goal.status,
                goal.priority
            ));
            if let Some(progress) = goal.progress {
                output.push_str(&format!("  Progress: {}\n", progress));
            }
        }
        Ok(ToolResult::ok(output))
    }
}

pub struct GoalUpdateTool {
    pub manager: Arc<GoalManager>,
}

#[async_trait]
impl Tool for GoalUpdateTool {
    fn name(&self) -> &str {
        "goal_update"
    }

    fn description(&self) -> &str {
        "Update the status or progress of an existing goal."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The ID of the goal to update."
                },
                "status": {
                    "type": "string",
                    "enum": ["Todo", "InProgress", "Completed", "Blocked", "Cancelled"],
                    "description": "The new status of the goal."
                },
                "progress": {
                    "type": "string",
                    "description": "A description of the current progress."
                }
            },
            "required": ["id"]
        }"#
        .to_string()
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let id = arguments["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing ID"))?;

        let status = arguments["status"].as_str().and_then(|s| match s {
            "Todo" => Some(GoalStatus::Todo),
            "InProgress" => Some(GoalStatus::InProgress),
            "Completed" => Some(GoalStatus::Completed),
            "Blocked" => Some(GoalStatus::Blocked),
            "Cancelled" => Some(GoalStatus::Cancelled),
            _ => None,
        });

        let progress = arguments["progress"].as_str().map(|s| s.to_string());

        self.manager.update_goal(id, status, progress)?;
        Ok(ToolResult::ok("Goal updated successfully."))
    }
}
