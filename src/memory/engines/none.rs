use crate::memory::{MemoryCategory, MemoryEntry, MemoryStore};
/// No-op memory backend — all writes succeed, all reads return empty.
/// Useful for testing or explicitly opting out of memory persistence.
use anyhow::Result;

pub struct NoneMemory;

impl MemoryStore for NoneMemory {
    fn name(&self) -> &str {
        "none"
    }

    fn store(
        &self,
        _key: &str,
        _content: &str,
        _category: MemoryCategory,
        _session_id: Option<&str>,
    ) -> Result<()> {
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

    fn get(&self, _key: &str) -> Result<Option<MemoryEntry>> {
        Ok(None)
    }

    fn list(
        &self,
        _category: Option<MemoryCategory>,
        _session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        Ok(vec![])
    }

    fn forget(&self, _key: &str) -> Result<bool> {
        Ok(false)
    }

    fn count(&self) -> Result<usize> {
        Ok(0)
    }

    fn health_check(&self) -> bool {
        true
    }
}
