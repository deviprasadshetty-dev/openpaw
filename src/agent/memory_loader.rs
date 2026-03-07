use anyhow::Result;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub key: String,
    pub content: String,
    pub session_id: Option<String>,
}

pub trait Memory: Send + Sync {
    fn store(&self, key: &str, content: &str, session_id: Option<&str>) -> Result<()>;
    fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>>;
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
}

/// Bridges any `crate::memory::MemoryStore` to the agent `Memory` trait.
/// This lets SqliteMemory, MarkdownMemory, LruMemory, etc. be passed to the agent.
pub struct MemoryAdapter<S: crate::memory::MemoryStore> {
    pub inner: Arc<S>,
}

impl<S: crate::memory::MemoryStore + Send + Sync + 'static> Memory for MemoryAdapter<S> {
    fn store(&self, key: &str, content: &str, session_id: Option<&str>) -> Result<()> {
        self.inner.store(
            key,
            content,
            crate::memory::MemoryCategory::Core,
            session_id,
            None,
        )
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
