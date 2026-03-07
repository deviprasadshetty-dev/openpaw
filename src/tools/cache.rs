use crate::agent::dispatcher::ToolExecutionResult;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct ToolCacheEntry {
    pub result: ToolExecutionResult,
    pub timestamp: Instant,
}

#[derive(Clone)]
pub struct ToolCache {
    entries: Arc<Mutex<HashMap<String, ToolCacheEntry>>>,
    pub ttl_secs: u64,
}

impl ToolCache {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            ttl_secs,
        }
    }

    pub fn get(&self, tool_name: &str, arguments_json: &str) -> Option<ToolExecutionResult> {
        let key = format!("{}:{}", tool_name, arguments_json);
        let mut cache = self.entries.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(entry) = cache.get(&key) {
            if entry.timestamp.elapsed() < Duration::from_secs(self.ttl_secs) {
                return Some(entry.result.clone());
            } else {
                // Expired
                cache.remove(&key);
            }
        }
        None
    }

    pub fn insert(&self, tool_name: &str, arguments_json: &str, result: ToolExecutionResult) {
        let key = format!("{}:{}", tool_name, arguments_json);
        let mut cache = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(
            key,
            ToolCacheEntry {
                result,
                timestamp: Instant::now(),
            },
        );
    }
}
