use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;

pub struct SkillInstallTool {
    pub workspace_dir: String,
}

#[async_trait]
impl Tool for SkillInstallTool {
    fn name(&self) -> &str {
        "skill_install"
    }

    fn description(&self) -> &str {
        "Install a skill from a GitHub URL into the workspace skills/ folder. The skill will be active on the next message. Use skill_search to find available skills first. For complex skills (like agent-browser), uses 'npx skills add' internally."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"url":{"type":"string","description":"GitHub repository URL of the skill to install, e.g. https://github.com/user/my-skill"},"name":{"type":"string","description":"Optional: custom folder name for the skill (defaults to the repo name)"}},"required":["url"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) if !u.trim().is_empty() => u.trim().to_string(),
            _ => return Ok(ToolResult::fail("Missing 'url' parameter")),
        };

        let derived_name = url
            .trim_end_matches('/')
            .split('/')
            .next_back()
            .unwrap_or("skill")
            .to_string();

        let skill_name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.trim().is_empty() => n.trim().to_string(),
            _ => derived_name,
        };

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

        tokio::fs::create_dir_all(&skills_dir).await?;

        if Path::new(&target).exists() {
            return Ok(ToolResult::fail(format!(
                "Skill '{}' is already installed at {}. Remove it first or use a different name.",
                skill_name, target
            )));
        }

        // Try git clone first
        let output = Command::new("git")
            .args(["clone", "--depth", "1", &url, &target])
            .output()
            .await;

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

        let has_skill_md = Path::new(&format!("{}/SKILL.md", target)).exists();
        let has_skill_toml = Path::new(&format!("{}/SKILL.toml", target)).exists();
        let has_skill_json = Path::new(&format!("{}/skill.json", target)).exists();

        if has_skill_md || has_skill_toml || has_skill_json {
            // Simple skill - ready to use
            return Ok(ToolResult::ok(format!(
                "✅ Skill '{}' installed at {}\nIt will be active on your next message — no restart needed.",
                skill_name, target
            )));
        }

        // Complex skill - check if it has .claude-plugin or needs npx skills
        let has_claude_plugin = Path::new(&format!("{}/.claude-plugin", target)).exists();
        let has_package_json = Path::new(&format!("{}/package.json", target)).exists();

        if has_claude_plugin || has_package_json {
            // This is a complex skill that needs 'npx skills add'
            // Remove the incomplete clone and use npx skills instead
            let _ = tokio::fs::remove_dir_all(&target).await;

            return match self.install_via_npx(&url, &skill_name, &skills_dir).await {
                Ok(msg) => Ok(ToolResult::ok(msg)),
                Err(e) => {
                    // Restore the git clone info for debugging
                    Ok(ToolResult::fail(format!(
                        "This appears to be a complex skill requiring 'npx skills add'.\n\
                         Manual installation: run 'npx skills add {} --skill {}'\n\
                         Error: {}",
                        url, skill_name, e
                    )))
                }
            };
        }

        // No recognizable skill structure
        let _ = tokio::fs::remove_dir_all(&target).await;
        Ok(ToolResult::fail(format!(
            "Installed repo at '{}' doesn't look like a skill — no SKILL.md, SKILL.toml, skill.json, or .claude-plugin found. Removed.",
            target
        )))
    }
}

impl SkillInstallTool {
    async fn install_via_npx(
        &self,
        url: &str,
        skill_name: &str,
        _skills_dir: &str,
    ) -> Result<String> {
        // Use npx skills add to install complex skills
        // Note: npx skills installs to .agent/ and .agents/ directories, not custom paths
        let output = Command::new("npx")
            .args([
                "skills", "add", url, "--skill", skill_name, "--yes", // Skip confirmation
            ])
            .output()
            .await?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(format!(
                "✅ Complex skill '{}' installed via npx skills.\n\
                 Note: Installed to .agent/ and .agents/ directories (managed by npx skills).\n\
                 {}\n\
                 It will be active on your next message.",
                skill_name, stdout
            ))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            Err(anyhow::anyhow!(
                "npx skills add failed: {}\n{}",
                stderr,
                stdout
            ))
        }
    }
}
