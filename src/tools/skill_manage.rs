use crate::tools::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

pub struct SkillManageTool {
    pub workspace_dir: String,
}

impl SkillManageTool {
    fn validate_skill_name(name: &str) -> Result<(), ToolResult> {
        if name.is_empty() {
            return Err(ToolResult::fail("Missing 'name' parameter"));
        }
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(ToolResult::fail("Skill name must be a simple alphanumeric string without path separators"));
        }
        Ok(())
    }

    fn validate_subpath(file_path: &str) -> Result<(), ToolResult> {
        if file_path.contains("..") || file_path.starts_with('/') || file_path.starts_with('\\') {
            return Err(ToolResult::fail("Invalid file path: path traversal is not allowed"));
        }
        Ok(())
    }

    async fn security_scan(content: &str) -> Result<(), ToolResult> {
        // Basic security scan: reject obvious malicious patterns
        let lower = content.to_lowercase();
        let blocked = [
            "rm -rf /",
            "rm -rf ~",
            "rm -rf /*",
            "dd if=/dev/zero",
            "mkfs.",
            ":(){ :|:& };:",
            "> /dev/sda",
        ];
        for pattern in &blocked {
            if lower.contains(pattern) {
                return Err(ToolResult::fail(
                    format!("Security scan blocked: content contains dangerous pattern '{}'", pattern)
                ));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Tool for SkillManageTool {
    fn name(&self) -> &str {
        "skill_manage"
    }

    fn description(&self) -> &'static str {
        "Create, edit, patch, delete, or manage files within a local skill. \
         Skills are reusable procedural knowledge saved to workspace/skills/<name>/SKILL.md. \
         Use write_file/remove_file to add supporting docs under references/, templates/, scripts/, or assets/. \
         Use this to persist successful workflows, error recoveries, and user corrections."
    }

    fn parameters_json(&self) -> String {
        r#"{
  "type": "object",
  "properties": {
    "action": {
      "type": "string",
      "enum": ["create", "edit", "patch", "delete", "write_file", "remove_file"],
      "description": "Action to perform on the skill"
    },
    "name": {
      "type": "string",
      "description": "Short, safe directory name for the skill (e.g. 'git-clean')"
    },
    "content": {
      "type": "string",
      "description": "Full SKILL.md content with YAML frontmatter for create/edit. Patch fragment for patch. File content for write_file."
    },
    "old_text": {
      "type": "string",
      "description": "Text to replace (required for patch action)"
    },
    "file_path": {
      "type": "string",
      "description": "Relative path inside the skill directory for write_file/remove_file (e.g. 'references/api.md')"
    }
  },
  "required": ["action", "name"]
}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");

        if let Err(e) = Self::validate_skill_name(name) {
            return Ok(e);
        }

        let skills_dir = Path::new(&self.workspace_dir).join("skills");
        let skill_dir = skills_dir.join(name);
        let skill_md = skill_dir.join("SKILL.md");

        match action {
            "create" => {
                let content = match args.get("content").and_then(|v| v.as_str()) {
                    Some(c) => c,
                    None => return Ok(ToolResult::fail("Missing 'content' parameter for create")),
                };

                if content.len() > 100_000 {
                    return Ok(ToolResult::fail("Skill content exceeds 100,000 character limit"));
                }

                if let Err(e) = Self::security_scan(content).await {
                    return Ok(e);
                }

                // Validate YAML frontmatter
                if !content.trim_start().starts_with("---") {
                    return Ok(ToolResult::fail(
                        "SKILL.md must start with YAML frontmatter (---\\nname: ...\\ndescription: ...\\n---)"
                    ));
                }

                if let Err(e) = tokio::fs::create_dir_all(&skill_dir).await {
                    return Ok(ToolResult::fail(format!("Failed to create skill directory: {}", e)));
                }

                if let Err(e) = tokio::fs::write(&skill_md, content).await {
                    return Ok(ToolResult::fail(format!("Failed to write SKILL.md: {}", e)));
                }

                Ok(ToolResult::ok(format!("Skill '{}' created successfully at {}", name, skill_dir.display())))
            }
            "edit" => {
                let content = match args.get("content").and_then(|v| v.as_str()) {
                    Some(c) => c,
                    None => return Ok(ToolResult::fail("Missing 'content' parameter for edit")),
                };

                if !skill_md.exists() {
                    return Ok(ToolResult::fail(format!("Skill '{}' does not exist. Use action='create' first.", name)));
                }

                if let Err(e) = tokio::fs::write(&skill_md, content).await {
                    return Ok(ToolResult::fail(format!("Failed to write SKILL.md: {}", e)));
                }

                Ok(ToolResult::ok(format!("Skill '{}' updated successfully.", name)))
            }
            "patch" => {
                let old_text = match args.get("old_text").and_then(|v| v.as_str()) {
                    Some(t) => t,
                    None => return Ok(ToolResult::fail("Missing 'old_text' parameter for patch")),
                };
                let new_text = match args.get("content").and_then(|v| v.as_str()) {
                    Some(t) => t,
                    None => return Ok(ToolResult::fail("Missing 'content' parameter for patch")),
                };

                if !skill_md.exists() {
                    return Ok(ToolResult::fail(format!("Skill '{}' does not exist.", name)));
                }

                let existing = match tokio::fs::read_to_string(&skill_md).await {
                    Ok(s) => s,
                    Err(e) => return Ok(ToolResult::fail(format!("Failed to read SKILL.md: {}", e))),
                };

                if !existing.contains(old_text) {
                    return Ok(ToolResult::fail(
                        format!("old_text not found in SKILL.md. The file may have changed.")
                    ));
                }

                let updated = existing.replace(old_text, new_text);
                if let Err(e) = tokio::fs::write(&skill_md, updated).await {
                    return Ok(ToolResult::fail(format!("Failed to write SKILL.md: {}", e)));
                }

                Ok(ToolResult::ok(format!("Skill '{}' patched successfully.", name)))
            }
            "delete" => {
                if !skill_dir.exists() {
                    return Ok(ToolResult::fail(format!("Skill '{}' does not exist.", name)));
                }

                if let Err(e) = tokio::fs::remove_dir_all(&skill_dir).await {
                    return Ok(ToolResult::fail(format!("Failed to delete skill directory: {}", e)));
                }

                Ok(ToolResult::ok(format!("Skill '{}' deleted successfully.", name)))
            }
            "write_file" => {
                let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
                    Some(fp) => fp,
                    None => return Ok(ToolResult::fail("Missing 'file_path' parameter for write_file")),
                };
                let content = match args.get("content").and_then(|v| v.as_str()) {
                    Some(c) => c,
                    None => return Ok(ToolResult::fail("Missing 'content' parameter for write_file")),
                };

                if let Err(e) = Self::validate_subpath(file_path) {
                    return Ok(e);
                }
                if let Err(e) = Self::security_scan(content).await {
                    return Ok(e);
                }

                if !skill_dir.exists() {
                    return Ok(ToolResult::fail(format!("Skill '{}' does not exist. Use action='create' first.", name)));
                }

                let target = skill_dir.join(file_path);
                if let Some(parent) = target.parent() {
                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                        return Ok(ToolResult::fail(format!("Failed to create parent directory: {}", e)));
                    }
                }

                if let Err(e) = tokio::fs::write(&target, content).await {
                    return Ok(ToolResult::fail(format!("Failed to write file: {}", e)));
                }

                Ok(ToolResult::ok(format!("Wrote file '{}' in skill '{}'.", file_path, name)))
            }
            "remove_file" => {
                let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
                    Some(fp) => fp,
                    None => return Ok(ToolResult::fail("Missing 'file_path' parameter for remove_file")),
                };

                if let Err(e) = Self::validate_subpath(file_path) {
                    return Ok(e);
                }

                let target = skill_dir.join(file_path);
                if !target.exists() {
                    return Ok(ToolResult::fail(format!("File '{}' does not exist in skill '{}'.", file_path, name)));
                }

                if let Err(e) = tokio::fs::remove_file(&target).await {
                    return Ok(ToolResult::fail(format!("Failed to remove file: {}", e)));
                }

                Ok(ToolResult::ok(format!("Removed file '{}' from skill '{}'.", file_path, name)))
            }
            _ => Ok(ToolResult::fail(format!("Unknown action: {}. Use create, edit, patch, delete, write_file, or remove_file.", action))),
        }
    }
}
