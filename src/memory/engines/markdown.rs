/// Markdown file memory backend.
///
/// Stores entries as `## key\n\ncontent\n\n---\n` sections in MEMORY.md.
/// 100% human-readable, no database required.
use anyhow::Result;
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::memory::{MemoryCategory, MemoryEntry, MemoryStore};

pub struct MarkdownMemory {
    path: PathBuf,
    lock: Mutex<()>,
}

impl MarkdownMemory {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    pub fn from_workspace(workspace_dir: &str) -> Self {
        let path = Path::new(workspace_dir).join("MEMORY.md");
        Self::new(path)
    }

    fn read_all(&self) -> Result<Vec<ParsedEntry>> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let content = fs::read_to_string(&self.path)?;
        Ok(parse_markdown(&content))
    }

    fn write_all(&self, entries: &[ParsedEntry]) -> Result<()> {
        let mut content = String::new();
        for e in entries {
            content.push_str(&format!("## {}\n\n{}\n\n---\n\n", e.key, e.content.trim()));
        }
        fs::write(&self.path, content)?;
        Ok(())
    }
}

struct ParsedEntry {
    key: String,
    content: String,
    timestamp: String,
}

impl MemoryStore for MarkdownMemory {
    fn name(&self) -> &str {
        "markdown"
    }

    fn store(
        &self,
        key: &str,
        content: &str,
        _category: MemoryCategory,
        _session_id: Option<&str>,
    ) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut entries = self.read_all()?;

        let ts = Utc::now().to_rfc3339();
        if let Some(e) = entries.iter_mut().find(|e| e.key == key) {
            e.content = content.to_string();
            e.timestamp = ts;
        } else {
            entries.push(ParsedEntry {
                key: key.to_string(),
                content: content.to_string(),
                timestamp: ts,
            });
        }
        self.write_all(&entries)
    }

    fn recall(
        &self,
        query: &str,
        limit: usize,
        _session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        let q = query.to_lowercase();
        let entries = self.read_all()?;
        let mut results: Vec<MemoryEntry> = entries
            .iter()
            .filter(|e| e.key.to_lowercase().contains(&q) || e.content.to_lowercase().contains(&q))
            .map(parsed_to_entry)
            .collect();
        results.truncate(limit);
        Ok(results)
    }

    fn get(&self, key: &str) -> Result<Option<MemoryEntry>> {
        Ok(self
            .read_all()?
            .into_iter()
            .find(|e| e.key == key)
            .map(|e| parsed_to_entry(&e)))
    }

    fn list(
        &self,
        _category: Option<MemoryCategory>,
        _session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        Ok(self.read_all()?.iter().map(parsed_to_entry).collect())
    }

    fn forget(&self, key: &str) -> Result<bool> {
        let _g = self.lock.lock().unwrap();
        let mut entries = self.read_all()?;
        let before = entries.len();
        entries.retain(|e| e.key != key);
        let removed = entries.len() < before;
        if removed {
            self.write_all(&entries)?;
        }
        Ok(removed)
    }

    fn count(&self) -> Result<usize> {
        Ok(self.read_all()?.len())
    }

    fn health_check(&self) -> bool {
        // Check if we can write to the directory
        if let Some(parent) = self.path.parent() {
            parent.exists()
        } else {
            true
        }
    }
}

/// Parse `## key\n\ncontent\n\n---` sections from markdown.
fn parse_markdown(content: &str) -> Vec<ParsedEntry> {
    let mut entries = Vec::new();
    let mut current_key: Option<String> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            // Save previous entry
            if let Some(key) = current_key.take() {
                let body = current_lines
                    .iter()
                    .map(|l| l.trim_end())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_string();
                // Remove trailing separator
                let body = body.trim_end_matches("---").trim().to_string();
                if !body.is_empty() {
                    entries.push(ParsedEntry {
                        key,
                        content: body,
                        timestamp: String::new(),
                    });
                }
                current_lines.clear();
            }
            current_key = Some(rest.trim().to_string());
        } else if current_key.is_some() {
            current_lines.push(line);
        }
    }

    // Save last entry
    if let Some(key) = current_key {
        let body = current_lines
            .iter()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        let body = body.trim_end_matches("---").trim().to_string();
        if !body.is_empty() {
            entries.push(ParsedEntry {
                key,
                content: body,
                timestamp: String::new(),
            });
        }
    }

    entries
}

fn parsed_to_entry(e: &ParsedEntry) -> MemoryEntry {
    MemoryEntry {
        id: e.key.clone(),
        key: e.key.clone(),
        content: e.content.clone(),
        category: MemoryCategory::Core,
        timestamp: e.timestamp.clone(),
        session_id: None,
        score: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty() {
        assert!(parse_markdown("").is_empty());
    }

    #[test]
    fn test_parse_single_entry() {
        let md = "## my_key\n\nhello world\n\n---\n\n";
        let entries = parse_markdown(md);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "my_key");
        assert_eq!(entries[0].content, "hello world");
    }

    #[test]
    fn test_parse_two_entries() {
        let md = "## key1\n\ncontent1\n\n---\n\n## key2\n\ncontent2\n\n---\n\n";
        let entries = parse_markdown(md);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, "key1");
        assert_eq!(entries[1].key, "key2");
    }
}
