use super::{Tool, ToolResult};
use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

/// Install a skill from a GitHub URL by git clone --depth 1 into workspace/skills/NAME.
/// Matches Nullclaw's Integrate phase: clone, verify structure, normalize name.
pub struct SkillInstallTool {
    pub workspace_dir: String,
}

impl Tool for SkillInstallTool {
    fn name(&self) -> &str {
        "skill_install"
    }

    fn description(&self) -> &str {
        "Install a skill from a GitHub URL into the workspace skills/ folder. The skill will be active on the next message. Use skill_search to find available skills first."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"url":{"type":"string","description":"GitHub repository URL of the skill to install, e.g. https://github.com/user/my-skill"},"name":{"type":"string","description":"Optional: custom folder name for the skill (defaults to the repo name)"}},"required":["url"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) if !u.trim().is_empty() => u.trim().to_string(),
            _ => return Ok(ToolResult::fail("Missing 'url' parameter")),
        };

        // Derive skill name from URL or use override
        let derived_name = url
            .trim_end_matches('/')
            .split('/')
            .last()
            .unwrap_or("skill")
            .to_string();

        let skill_name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.trim().is_empty() => n.trim().to_string(),
            _ => derived_name,
        };

        // Sanitize: no path separators, no "..", no NUL
        if skill_name.contains('/')
            || skill_name.contains('\\')
            || skill_name.contains('\0')
            || skill_name == ".."
        {
            return Ok(ToolResult::fail(format!(
                "Unsafe skill name: '{}'",
                skill_name
            )));
        }

        let skills_dir = format!("{}/skills", self.workspace_dir);
        let target = format!("{}/{}", skills_dir, skill_name);

        // Create skills/ directory if needed
        std::fs::create_dir_all(&skills_dir)?;

        // Don't overwrite an existing skill without explicit name override
        if Path::new(&target).exists() {
            return Ok(ToolResult::fail(format!(
                "Skill '{}' is already installed at {}. Remove it first or use a different name.",
                skill_name, target
            )));
        }

        // git clone --depth 1 <url> <target>
        let output = Command::new("git")
            .args(["clone", "--depth", "1", &url, &target])
            .output();

        match output {
            Err(e) => {
                return Ok(ToolResult::fail(format!(
                    "Failed to run git: {}. Is git installed?",
                    e
                )));
            }
            Ok(out) if !out.status.success() => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Ok(ToolResult::fail(format!(
                    "git clone failed: {}",
                    stderr.trim()
                )));
            }
            Ok(_) => {}
        }

        // Verify the cloned repo has at least one expected structure file
        let has_skill_md = Path::new(&format!("{}/SKILL.md", target)).exists();
        let has_skill_toml = Path::new(&format!("{}/SKILL.toml", target)).exists();
        let has_skill_json = Path::new(&format!("{}/skill.json", target)).exists();

        if !has_skill_md && !has_skill_toml && !has_skill_json {
            // Not a valid skill — remove and reject
            let _ = std::fs::remove_dir_all(&target);
            return Ok(ToolResult::fail(format!(
                "Installed repo at '{}' doesn't look like a skill — no SKILL.md, SKILL.toml, or skill.json found. Removed.",
                target
            )));
        }

        Ok(ToolResult::ok(format!(
            "✅ Skill '{}' installed at {}\nIt will be active on your next message — no restart needed.",
            skill_name, target
        )))
    }
}
