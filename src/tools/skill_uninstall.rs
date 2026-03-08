use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

/// Uninstall a skill from the workspace/skills/ folder.
pub struct SkillUninstallTool {
    pub workspace_dir: String,
}

impl Tool for SkillUninstallTool {
    fn name(&self) -> &str {
        "skill_uninstall"
    }

    fn description(&self) -> &str {
        "Uninstall (remove) a skill by its name from the workspace. This will permanently delete the skill folder."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"name":{"type":"string","description":"Name of the skill to uninstall (the folder name in workspace/skills/)"}},"required":["name"]}"#.to_string()
    }

    fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.trim().is_empty() => n.trim(),
            _ => return Ok(ToolResult::fail("Missing 'name' parameter")),
        };

        // Sanitize: no path separators, no "..", no NUL
        if name.contains('/') || name.contains('\\') || name.contains('\0') || name == ".." {
            return Ok(ToolResult::fail(format!("Unsafe skill name: '{}'", name)));
        }

        let skill_path = Path::new(&self.workspace_dir).join("skills").join(name);

        if !skill_path.exists() {
            return Ok(ToolResult::fail(format!(
                "Skill '{}' not found at {:?}. It might be a built-in skill or already removed.",
                name, skill_path
            )));
        }

        if !skill_path.is_dir() {
            return Ok(ToolResult::fail(format!(
                "Path {:?} exists but is not a directory.",
                skill_path
            )));
        }

        // Final check: is it actually in the workspace?
        let canonical_workspace = fs::canonicalize(&self.workspace_dir)
            .unwrap_or_else(|_| self.workspace_dir.clone().into());
        let canonical_skill = fs::canonicalize(&skill_path).unwrap_or_else(|_| skill_path.clone());

        if !canonical_skill.starts_with(&canonical_workspace) {
            return Ok(ToolResult::fail(
                "Attempted to delete a directory outside the workspace.",
            ));
        }

        std::fs::remove_dir_all(&skill_path)?;

        Ok(ToolResult::ok(format!(
            "✅ Skill '{}' has been uninstalled and its folder removed.",
            name
        )))
    }
}

use std::fs;
