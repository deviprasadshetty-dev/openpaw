use anyhow::Result;

#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub key: String,
    pub content: String,
    pub session_id: Option<String>,
}

pub trait Memory: Send + Sync {
    fn store(&self, key: &str, content: &str, session_id: Option<&str>) -> Result<()>;
    fn recall(&self, query: &str, limit: usize, session_id: Option<&str>) -> Result<Vec<MemoryEntry>>;
}

pub struct NoopMemory;

impl Memory for NoopMemory {
    fn store(&self, _key: &str, _content: &str, _session_id: Option<&str>) -> Result<()> {
        Ok(())
    }
    fn recall(&self, _query: &str, _limit: usize, _session_id: Option<&str>) -> Result<Vec<MemoryEntry>> {
        Ok(vec![])
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
    context.push_str("\n");
    context.push_str(user_message);
    
    Ok(context)
}
