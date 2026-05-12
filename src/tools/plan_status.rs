use super::{Tool, ToolContext, ToolResult};
use crate::plan::PlanManager;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct PlanStatusTool {
    pub plan_manager: Arc<PlanManager>,
}

#[async_trait]
impl Tool for PlanStatusTool {
    fn name(&self) -> &str {
        "plan_status"
    }

    fn description(&self) -> &str {
        "Show the current status, task outcomes, and final review summary for a background plan."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "plan_id": {
                    "type": "integer",
                    "description": "Plan id returned by plan_create"
                }
            },
            "required": ["plan_id"]
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let plan_id = match args.get("plan_id").and_then(|v| v.as_u64()) {
            Some(id) => id,
            None => return Ok(ToolResult::fail("Missing required 'plan_id' parameter")),
        };

        match self.plan_manager.get_status(plan_id) {
            Some(status) => Ok(ToolResult::ok(status)),
            None => Ok(ToolResult::fail(format!("Plan {} not found.", plan_id))),
        }
    }
}
