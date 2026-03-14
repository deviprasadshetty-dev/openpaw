use anyhow::Result;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub key: String,
    pub content: String,
    pub session_id: Option<String>,
}

pub trait Memory: Send + Sync {
    fn as_any(&self) -> &dyn std::any::Any;
    fn store(&self, key: &str, content: &str, session_id: Option<&str>) -> Result<()>;
    fn get(&self, key: &str) -> Result<Option<MemoryEntry>>;
    fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>>;
    fn list(&self, session_id: Option<&str>) -> Result<Vec<MemoryEntry>>;
    fn forget(&self, key: &str) -> Result<bool>;
    fn semantic_recall(
        &self,
        _query: &str,
        _limit: usize,
        _session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        Ok(vec![])
    }
}

pub struct NoopMemory;

impl Memory for NoopMemory {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn get(&self, _key: &str) -> Result<Option<MemoryEntry>> {
        Ok(None)
    }
    fn store(&self, _key: &str, _content: &str, _session_id: Option<&str>) -> Result<()> {
        Ok(())
    }
    fn recall(
        &self,
        _query: &str,
        _limit: usize,
        _session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        Ok(vec![])
    }
    fn list(&self, _session_id: Option<&str>) -> Result<Vec<MemoryEntry>> {
        Ok(vec![])
    }
    fn forget(&self, _key: &str) -> Result<bool> {
        Ok(false)
    }
}

/// Bridges any `crate::memory::MemoryStore` to the agent `Memory` trait.
/// This lets SqliteMemory, MarkdownMemory, LruMemory, etc. be passed to the agent.
pub struct MemoryAdapter<S: crate::memory::MemoryStore> {
    pub inner: Arc<S>,
}

impl<S: crate::memory::MemoryStore + Send + Sync + 'static> Memory for MemoryAdapter<S> {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn store(&self, key: &str, content: &str, session_id: Option<&str>) -> Result<()> {
        self.inner.store(
            key,
            content,
            crate::memory::MemoryCategory::Core,
            session_id,
            None,
        )
    }

    fn get(&self, key: &str) -> Result<Option<MemoryEntry>> {
        let entry = self.inner.get(key)?;
        Ok(entry.map(|e| MemoryEntry {
            key: e.key,
            content: e.content,
            session_id: e.session_id,
        }))
    }

    fn forget(&self, key: &str) -> Result<bool> {
        self.inner.forget(key)
    }

    fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        let entries = self.inner.recall(query, limit, session_id)?;
        Ok(entries
            .into_iter()
            .map(|e| MemoryEntry {
                key: e.key,
                content: e.content,
                session_id: e.session_id,
            })
            .collect())
    }

    fn list(&self, session_id: Option<&str>) -> Result<Vec<MemoryEntry>> {
        let entries = self.inner.recall("", 1000, session_id)?;
        Ok(entries
            .into_iter()
            .map(|e| MemoryEntry {
                key: e.key,
                content: e.content,
                session_id: e.session_id,
            })
            .collect())
    }

    fn semantic_recall(
        &self,
        query: &str,
        limit: usize,
        _session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        let entries = self.inner.semantic_recall_by_text(query, limit)?;
        Ok(entries
            .into_iter()
            .map(|e| MemoryEntry {
                key: e.key,
                content: e.content,
                session_id: e.session_id,
            })
            .collect())
    }
}

pub fn enrich_message(
    memory: &dyn Memory,
    user_message: &str,
    session_id: Option<&str>,
) -> Result<String> {
    // Skip enrichment for very short or trivial messages to save prompt tokens
    let trimmed = user_message.trim();
    if trimmed.len() < 10 {
        return Ok(user_message.to_string());
    }

    // Skip enrichment for conversational/follow-up messages to prevent
    // agent confusion between past and current instructions
    let lower = trimmed.to_lowercase();
    if lower.contains("where")
        || lower.contains("what")
        || lower.contains("when")
        || lower.contains("why")
        || lower.contains("how")
        || lower.contains("?")
        || lower.starts_with("did ")
        || lower.starts_with("do ")
        || lower.starts_with("does ")
        || lower.starts_with("is ")
        || lower.starts_with("are ")
        || lower.starts_with("was ")
        || lower.starts_with("were ")
        || lower.starts_with("can ")
        || lower.starts_with("could ")
        || lower.starts_with("should ")
        || lower.starts_with("would ")
    {
        // Conversational question - don't enrich with old task memories
        return Ok(user_message.to_string());
    }

    let entries = memory.recall(user_message, 5, session_id)?;
    if entries.is_empty() {
        return Ok(user_message.to_string());
    }

    let mut context = String::from("[Memory context]\n");
    for entry in entries {
        // Simple sanitization/formatting
        context.push_str(&format!("- {}: {}\n", entry.key, entry.content));
    }
    context.push('\n');
    context.push_str(user_message);

    Ok(context)
}
