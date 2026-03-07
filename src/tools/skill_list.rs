use super::{Tool, ToolResult};
use crate::skills::list_skills_merged;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

/// List all installed and built-in skills.
pub struct SkillListTool {
    pub workspace_dir: String,
    pub builtin_dir: String,
}

impl Tool for SkillListTool {
    fn name(&self) -> &str {
        "skill_list"
    }

    fn description(&self) -> &str {
        "List all installed skills, showing their versions, descriptions, and whether their dependencies (bins/env) are met."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{},"required":[]}"#.to_string()
    }

    fn execute(&self, _args: Value) -> Result<ToolResult> {
        let skills =
            list_skills_merged(Path::new(&self.builtin_dir), Path::new(&self.workspace_dir))?;

        if skills.is_empty() {
            return Ok(ToolResult::ok(
                "No skills installed. Use skill_search to find some!",
            ));
        }

        let mut lines = vec![format!("Total skills: {}\n", skills.len())];

        for (i, skill) in skills.iter().enumerate() {
            let status = if skill.available {
                "✅ Ready"
            } else {
                &format!("❌ Missing deps: {}", skill.missing_deps)
            };

            let source =
                if skill.path.contains("workspace/skills") || skill.path.contains("/skills/") {
                    "Installed"
                } else {
                    "Built-in"
                };

            lines.push(format!(
                "{}. **{}** (v{}) [{}]\n   {}\n   Status: {}\n   Path: {}\n",
                i + 1,
                skill.name,
                skill.version,
                source,
                if skill.description.is_empty() {
                    "No description"
                } else {
                    &skill.description
                },
                status,
                skill.path
            ));
        }

        Ok(ToolResult::ok(lines.join("\n")))
    }
}
