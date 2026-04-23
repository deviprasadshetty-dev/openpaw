use crate::tools::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

pub struct SkillViewTool {
    pub workspace_dir: String,
    pub builtin_dir: String,
}

#[async_trait]
impl Tool for SkillViewTool {
    fn name(&self) -> &str {
        "skill_view"
    }

    fn description(&self) -> &str {
        "View the full content of a specific skill by name. \
         Returns the SKILL.md contents, metadata, and any linked files. \
         Use this to recall procedural knowledge before starting a task."
    }

    fn parameters_json(&self) -> String {
        r#"{
  "type": "object",
  "properties": {
    "name": {
      "type": "string",
      "description": "Name of the skill to view"
    },
    "file_path": {
      "type": "string",
      "description": "Optional: relative path to a linked file inside the skill directory"
    }
  },
  "required": ["name"]
}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let file_path = args.get("file_path").and_then(|v| v.as_str());

        if name.is_empty() {
            return Ok(ToolResult::fail("Missing 'name' parameter"));
        }
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return Ok(ToolResult::fail("Skill name must be a simple alphanumeric string"));
        }

        // Search workspace skills first, then builtin
        let search_dirs = [
            Path::new(&self.workspace_dir).join("skills"),
            Path::new(&self.builtin_dir).join("skills"),
        ];

        let mut found_dir = None;
        for dir in &search_dirs {
            let candidate = dir.join(name);
            if candidate.is_dir() && candidate.join("SKILL.md").exists() {
                found_dir = Some(candidate);
                break;
            }
        }

        let skill_dir = match found_dir {
            Some(d) => d,
            None => {
                return Ok(ToolResult::fail(format!(
                    "Skill '{}' not found in workspace or builtin skills.",
                    name
                )));
            }
        };

        // If a specific file_path is requested, validate and return it
        if let Some(fp) = file_path {
            if fp.contains("..") {
                return Ok(ToolResult::fail("Path traversal is not allowed"));
            }
            let target = skill_dir.join(fp);
            // Ensure the resolved path is still inside skill_dir
            let canonical_target = match tokio::fs::canonicalize(&target).await {
                Ok(p) => p,
                Err(_) => {
                    return Ok(ToolResult::fail(format!(
                        "Linked file '{}' not found in skill '{}'.",
                        fp, name
                    )));
                }
            };
            let canonical_skill_dir = match tokio::fs::canonicalize(&skill_dir).await {
                Ok(p) => p,
                Err(e) => return Ok(ToolResult::fail(format!("Failed to canonicalize skill dir: {}", e))),
            };
            if !canonical_target.starts_with(&canonical_skill_dir) {
                return Ok(ToolResult::fail("Path traversal detected"));
            }

            let content = match tokio::fs::read_to_string(&canonical_target).await {
                Ok(c) => c,
                Err(e) => return Ok(ToolResult::fail(format!("Failed to read file: {}", e))),
            };

            return Ok(ToolResult::ok(format!(
                "Skill: {}\nFile: {}\n\n{}",
                name, fp, content
            )));
        }

        // Return SKILL.md content + metadata
        let skill_md = skill_dir.join("SKILL.md");
        let content = match tokio::fs::read_to_string(&skill_md).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::fail(format!("Failed to read SKILL.md: {}", e))),
        };

        // List linked files (anything else in the directory)
        let mut linked_files = Vec::new();
        let mut entries = match tokio::fs::read_dir(&skill_dir).await {
            Ok(e) => e,
            Err(_) => {
                return Ok(ToolResult::ok(format!(
                    "Skill: {}\nLocation: {}\n\n{}",
                    name,
                    skill_dir.display(),
                    content
                )));
            }
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let fname = entry.file_name().to_string_lossy().to_string();
            if fname != "SKILL.md" {
                linked_files.push(fname);
            }
        }

        let mut result = format!(
            "Skill: {}\nLocation: {}\n\n{}",
            name,
            skill_dir.display(),
            content
        );

        if !linked_files.is_empty() {
            result.push_str("\n\nLinked files:\n");
            for f in linked_files {
                result.push_str(&format!("- {}\n", f));
            }
        }

        Ok(ToolResult::ok(result))
    }
}
