pub mod embeddings;
pub mod engines;
pub mod postgres;
pub mod sqlite;

use anyhow::Result;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryCategory {
    Core,
    Working,
    Archival,
    Tool,
}

impl fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryCategory::Core => write!(f, "core"),
            MemoryCategory::Working => write!(f, "working"),
            MemoryCategory::Archival => write!(f, "archival"),
            MemoryCategory::Tool => write!(f, "tool"),
        }
    }
}

impl FromStr for MemoryCategory {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "core" => Ok(MemoryCategory::Core),
            "working" => Ok(MemoryCategory::Working),
            "archival" => Ok(MemoryCategory::Archival),
            "tool" => Ok(MemoryCategory::Tool),
            _ => Ok(MemoryCategory::Core), // Default fallback
        }
    }
}

impl MemoryCategory {
    pub fn from_str(s: &str) -> Self {
        <Self as std::str::FromStr>::from_str(s).unwrap_or(MemoryCategory::Core)
    }
}

#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: String,
    pub key: String,
    pub content: String,
    pub category: MemoryCategory,
    pub timestamp: String,
    pub session_id: Option<String>,
    pub score: Option<f64>,
    pub importance: f64,
    pub embedding: Option<Vec<f32>>,
}

pub trait MemoryStore: Send + Sync {
    fn name(&self) -> &str;
    fn store(
        &self,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
        importance: Option<f64>,
    ) -> Result<()>;
    fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>>;
    fn get(&self, key: &str) -> Result<Option<MemoryEntry>>;
    fn list(
        &self,
        category: Option<MemoryCategory>,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>>;
    fn forget(&self, key: &str) -> Result<bool>;
    fn count(&self) -> Result<usize>;
    fn health_check(&self) -> bool;

    // Optional methods for advanced memory functionality
    fn semantic_recall(&self, _embedding: &[f32], _limit: usize) -> Result<Vec<MemoryEntry>> {
        Ok(Vec::new())
    }

    fn semantic_recall_by_text(&self, _query: &str, _limit: usize) -> Result<Vec<MemoryEntry>> {
        Ok(Vec::new())
    }

    fn decay_importance(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct MessageEntry {
    pub role: String,
    pub content: String,
}

pub trait SessionStore: Send + Sync {
    fn save_message(&self, session_id: &str, role: &str, content: &str) -> Result<()>;
    fn load_messages(&self, session_id: &str) -> Result<Vec<MessageEntry>>;
    fn clear_messages(&self, session_id: &str) -> Result<()>;
    fn clear_autosaved(&self, session_id: Option<&str>) -> Result<()>;
}
