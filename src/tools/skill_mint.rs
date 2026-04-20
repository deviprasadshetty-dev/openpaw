use crate::tools::Tool;
use crate::tools::ToolResult;
use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;

pub struct SkillMintTool {
    pub workspace_dir: String,
}

#[async_trait]
impl Tool for SkillMintTool {
    fn name(&self) -> &'static str {
        "skill_mint"
    }

    fn description(&self) -> &'static str {
        "Autonomously mint a new skill or executable tool. Use this instead of 'file_write' to create new Skill Mints. \
         It automatically handles the SKILL.md YAML frontmatter and folder creation. \
         If you provide script_name and script_content, it will create an executable tool mint."
    }

    fn parameters_json(&self) -> String {
        r#"{
  "type": "object",
  "properties": {
    "name": { "type": "string", "description": "Short, safe directory name for the skill (e.g. 'git-clean')" },
    "description": { "type": "string", "description": "When and why to trigger this skill" },
    "instructions": { "type": "string", "description": "The markdown instructions (the body of the skill)" },
    "script_name": { "type": "string", "description": "Optional: Name of the script file (e.g., 'helper.py' or 'run.sh')" },
    "script_content": { "type": "string", "description": "Optional: Content of the executable script" }
  },
  "required": ["name", "description", "instructions"]
}"#.to_string()
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _context: &crate::tools::ToolContext,
    ) -> Result<ToolResult> {
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.trim(),
            None => return Ok(ToolResult::fail("Missing 'name' argument")),
        };
        let desc = match args.get("description").and_then(|v| v.as_str()) {
            Some(d) => d.trim(),
            None => return Ok(ToolResult::fail("Missing 'description' argument")),
        };
        let instructions = match args.get("instructions").and_then(|v| v.as_str()) {
            Some(i) => i.trim(),
            None => return Ok(ToolResult::fail("Missing 'instructions' argument")),
        };

        if name.contains('/') || name.contains('\\') || name.contains('.') {
            return Ok(ToolResult::fail("Skill name must be a simple alphanumeric string without slashes or dots"));
        }

        let skills_dir = Path::new(&self.workspace_dir).join("skills");
        let target_dir = skills_dir.join(name);

        if let Err(e) = tokio::fs::create_dir_all(&target_dir).await {
            return Ok(ToolResult::fail(format!("Failed to create skill directory: {}", e)));
        }

        // Write SKILL.md
        let md_content = format!(
            "---\nname: {}\ndescription: {}\n---\n{}",
            name, desc, instructions
        );
        let md_path = target_dir.join("SKILL.md");
        if let Err(e) = tokio::fs::write(&md_path, md_content).await {
            return Ok(ToolResult::fail(format!("Failed to write SKILL.md: {}", e)));
        }

        // Handle executable script if provided
        let mut executable_msg = String::new();
        if let Some(script_name) = args.get("script_name").and_then(|v| v.as_str()) {
            if let Some(script_content) = args.get("script_content").and_then(|v| v.as_str()) {
                if !script_name.contains('/') && !script_name.contains('\\') && script_name != "." && script_name != ".." {
                    let script_path = target_dir.join(script_name);
                    let _ = tokio::fs::write(&script_path, script_content).await;
                    
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Ok(mut perms) = tokio::fs::metadata(&script_path).await.map(|m| m.permissions()) {
                            perms.set_mode(0o755);
                            let _ = tokio::fs::set_permissions(&script_path, perms).await;
                        }
                    }

                    // Create skill.json tool mapping
                    let command = if script_name.ends_with(".py") {
                        format!("python {}", script_name)
                    } else if script_name.ends_with(".sh") {
                        format!("bash {}", script_name)
                    } else if script_name.ends_with(".js") {
                        format!("node {}", script_name)
                    } else if script_name.ends_with(".ps1") {
                        format!("powershell -f {}", script_name)
                    } else {
                        format!("./{}", script_name)
                    };

                    let skill_json = format!(r#"{{
  "tools": [
    {{
      "name": "{}",
      "description": "{}",
      "command": "{}",
      "parameters": {{
        "type": "object",
        "properties": {{
          "args": {{
            "type": "string",
            "description": "Arguments to pass to the script"
          }}
        }},
        "required": []
      }}
    }}
  ]
}}"#, name, desc, command);
                    
                    let _ = tokio::fs::write(target_dir.join("skill.json"), skill_json).await;
                    executable_msg = format!(" Also created executable tool '{}' mapped to '{}'.", name, script_name);
                }
            }
        }

        Ok(ToolResult::ok(format!("Skill Mint '{}' successfully created at {}.{}", name, target_dir.display(), executable_msg)))
    }
}

