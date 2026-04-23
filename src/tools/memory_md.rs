use crate::tools::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

const ENTRY_DELIMITER: &str = "\n§\n";

pub struct MemoryMdTool {
    pub workspace_dir: String,
    pub memory_char_limit: usize,
    pub user_char_limit: usize,
}

impl MemoryMdTool {
    fn memory_file(&self) -> std::path::PathBuf {
        Path::new(&self.workspace_dir).join("MEMORY.md")
    }

    fn user_file(&self) -> std::path::PathBuf {
        Path::new(&self.workspace_dir).join("USER.md")
    }

    async fn read_entries(&self, path: &std::path::PathBuf) -> Vec<String> {
        match tokio::fs::read_to_string(path).await {
            Ok(content) if !content.trim().is_empty() => {
                content.split(ENTRY_DELIMITER).map(|s| s.to_string()).collect()
            }
            _ => Vec::new(),
        }
    }

    async fn write_entries(&self, path: &std::path::PathBuf, entries: &[String]) -> Result<()> {
        let content = if entries.is_empty() {
            ""
        } else {
            &entries.join(ENTRY_DELIMITER)
        };
        tokio::fs::write(path, content).await?;
        Ok(())
    }

    async fn current_len(&self, path: &std::path::PathBuf) -> usize {
        match tokio::fs::read_to_string(path).await {
            Ok(c) => c.len(),
            Err(_) => 0,
        }
    }
}

#[async_trait]
impl Tool for MemoryMdTool {
    fn name(&self) -> &str {
        "memory_md"
    }

    fn description(&self) -> &'static str {
        "Manage persistent markdown memory files: MEMORY.md (environment facts, project conventions) \
         and USER.md (user preferences, communication style). \
         Actions: add, replace, remove. Files have strict character limits. \
         Adding an entry that would exceed the limit returns an error — you must replace or remove existing entries first."
    }

    fn parameters_json(&self) -> String {
        r#"{
  "type": "object",
  "properties": {
    "action": {
      "type": "string",
      "enum": ["add", "replace", "remove"],
      "description": "Action to perform"
    },
    "target": {
      "type": "string",
      "enum": ["memory", "user"],
      "description": "Which file to target"
    },
    "content": {
      "type": "string",
      "description": "Content to add or replacement text"
    },
    "old_text": {
      "type": "string",
      "description": "Text to find and replace or remove"
    }
  },
  "required": ["action", "target"]
}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("memory");

        let (path, limit) = match target {
            "user" => (self.user_file(), self.user_char_limit),
            _ => (self.memory_file(), self.memory_char_limit),
        };

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        match action {
            "add" => {
                let content = match args.get("content").and_then(|v| v.as_str()) {
                    Some(c) => c,
                    None => return Ok(ToolResult::fail("Missing 'content' parameter for add")),
                };

                let mut entries = self.read_entries(&path).await;

                // Deduplication: reject exact duplicates
                if entries.iter().any(|e| e.trim() == content.trim()) {
                    return Ok(ToolResult::fail("Exact duplicate entry already exists."));
                }

                // Hard limit check
                let current = self.current_len(&path).await;
                let new_len = if current == 0 {
                    content.len()
                } else {
                    current + ENTRY_DELIMITER.len() + content.len()
                };
                if new_len > limit {
                    return Ok(ToolResult::fail(format!(
                        "Memory at {}/{} chars. Adding this entry would exceed the {} char limit. Replace or remove existing entries first.",
                        current, limit, limit
                    )));
                }

                entries.push(content.to_string());
                self.write_entries(&path, &entries).await?;

                Ok(ToolResult::ok(format!(
                    "Added entry to {} file. ({}/{} chars)",
                    target, new_len, limit
                )))
            }
            "replace" => {
                let old_text = match args.get("old_text").and_then(|v| v.as_str()) {
                    Some(t) => t,
                    None => return Ok(ToolResult::fail("Missing 'old_text' parameter for replace")),
                };
                let content = match args.get("content").and_then(|v| v.as_str()) {
                    Some(c) => c,
                    None => return Ok(ToolResult::fail("Missing 'content' parameter for replace")),
                };

                let file_content = match tokio::fs::read_to_string(&path).await {
                    Ok(c) => c,
                    Err(_) => return Ok(ToolResult::fail(format!("{} file does not exist.", target))),
                };

                if !file_content.contains(old_text) {
                    return Ok(ToolResult::fail("old_text not found in file."));
                }

                let updated = file_content.replace(old_text, content);
                if updated.len() > limit {
                    return Ok(ToolResult::fail(format!(
                        "Replace would result in {}/{} chars, exceeding the limit. Remove or consolidate entries first.",
                        updated.len(), limit
                    )));
                }

                tokio::fs::write(&path, updated).await?;

                Ok(ToolResult::ok(format!(
                    "Replaced text in {} file. ({}/{} chars)",
                    target,
                    self.current_len(&path).await,
                    limit
                )))
            }
            "remove" => {
                let old_text = match args.get("old_text").and_then(|v| v.as_str()) {
                    Some(t) => t,
                    None => return Ok(ToolResult::fail("Missing 'old_text' parameter for remove")),
                };

                let file_content = match tokio::fs::read_to_string(&path).await {
                    Ok(c) => c,
                    Err(_) => return Ok(ToolResult::fail(format!("{} file does not exist.", target))),
                };

                if !file_content.contains(old_text) {
                    return Ok(ToolResult::fail("old_text not found in file."));
                }

                let updated = file_content.replace(old_text, "");
                // Clean up double delimiters that may result
                let double_delim = format!("{}{}", ENTRY_DELIMITER, ENTRY_DELIMITER);
                let cleaned = updated.replace(&double_delim, ENTRY_DELIMITER);
                tokio::fs::write(&path, cleaned.trim()).await?;

                Ok(ToolResult::ok(format!("Removed text from {} file.", target)))
            }
            _ => Ok(ToolResult::fail(format!("Unknown action: {}. Use add, replace, or remove.", action))),
        }
    }
}
