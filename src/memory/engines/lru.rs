/// In-process LRU memory — fast, zero deps, no persistence.
/// Useful as a runtime cache layered on top of SQLite.
use anyhow::Result;
use chrono::Utc;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use crate::memory::{MemoryCategory, MemoryEntry, MemoryStore};

struct Entry {
    key: String,
    content: String,
    category: MemoryCategory,
    timestamp: String,
    session_id: Option<String>,
}

pub struct LruMemory {
    capacity: usize,
    inner: Mutex<LruInner>,
}

struct LruInner {
    map: HashMap<String, Entry>,
    order: VecDeque<String>, // front = most recently used
}

impl LruMemory {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            inner: Mutex::new(LruInner {
                map: HashMap::new(),
                order: VecDeque::new(),
            }),
        }
    }

    fn touch(inner: &mut LruInner, key: &str) {
        if let Some(pos) = inner.order.iter().position(|k| k == key) {
            inner.order.remove(pos);
        }
        inner.order.push_front(key.to_string());
    }
}

impl Default for LruMemory {
    fn default() -> Self {
        Self::new(512)
    }
}

impl MemoryStore for LruMemory {
    fn name(&self) -> &str {
        "lru"
    }

    fn store(
        &self,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
        _importance: Option<f64>,
    ) -> Result<()> {
        let mut g = self.inner.lock().unwrap();

        if g.map.contains_key(key) {
            // Update in-place
            if let Some(e) = g.map.get_mut(key) {
                e.content = content.to_string();
                e.category = category;
            }
            Self::touch(&mut g, key);
        } else {
            // Evict if at capacity
            if g.map.len() >= self.capacity
                && let Some(old) = g.order.pop_back()
            {
                g.map.remove(&old);
            }
            g.map.insert(
                key.to_string(),
                Entry {
                    key: key.to_string(),
                    content: content.to_string(),
                    category,
                    timestamp: Utc::now().to_rfc3339(),
                    session_id: session_id.map(str::to_string),
                },
            );
            g.order.push_front(key.to_string());
        }
        Ok(())
    }

    fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        let g = self.inner.lock().unwrap();
        let q = query.to_lowercase();

        let mut results: Vec<MemoryEntry> = g
            .map
            .values()
            .filter(|e| {
                let session_ok = session_id.is_none_or(|sid| e.session_id.as_deref() == Some(sid));
                session_ok && e.content.to_lowercase().contains(&q)
            })
            .map(to_entry)
            .collect();

        results.truncate(limit);
        Ok(results)
    }

    fn get(&self, key: &str) -> Result<Option<MemoryEntry>> {
        let g = self.inner.lock().unwrap();
        Ok(g.map.get(key).map(to_entry))
    }

    fn list(
        &self,
        category: Option<MemoryCategory>,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        let g = self.inner.lock().unwrap();
        Ok(g.map
            .values()
            .filter(|e| {
                let cat_ok = category.is_none_or(|c| e.category == c);
                let sid_ok = session_id.is_none_or(|sid| e.session_id.as_deref() == Some(sid));
                cat_ok && sid_ok
            })
            .map(to_entry)
            .collect())
    }

    fn forget(&self, key: &str) -> Result<bool> {
        let mut g = self.inner.lock().unwrap();
        if g.map.remove(key).is_some() {
            if let Some(pos) = g.order.iter().position(|k| k == key) {
                g.order.remove(pos);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn count(&self) -> Result<usize> {
        Ok(self.inner.lock().unwrap().map.len())
    }

    fn health_check(&self) -> bool {
        true
    }
}

fn to_entry(e: &Entry) -> MemoryEntry {
    MemoryEntry {
        id: e.key.clone(),
        key: e.key.clone(),
        content: e.content.clone(),
        category: e.category,
        timestamp: e.timestamp.clone(),
        session_id: e.session_id.clone(),
        score: None,
        importance: 0.5,
        embedding: None,
    }
}
