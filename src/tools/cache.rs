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
    pub max_entries: usize,
}

impl ToolCache {
    pub fn new(ttl_secs: u64, max_entries: usize) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            ttl_secs,
            max_entries,
        }
    }

    pub fn get(&self, tool_name: &str, arguments_json: &str) -> Option<ToolExecutionResult> {
        let key = format!("{}:{}", tool_name, arguments_json);
        let mut cache = self.entries.lock().unwrap_or_else(|e| e.into_inner());

        let ttl = self.tool_specific_ttl(tool_name);

        if let Some(entry) = cache.get(&key) {
            if entry.timestamp.elapsed() < Duration::from_secs(ttl) {
                return Some(entry.result.clone());
            } else {
                // Expired
                cache.remove(&key);
            }
        }
        None
    }

    fn tool_specific_ttl(&self, tool_name: &str) -> u64 {
        match tool_name {
            "memory_recall" => 600,      // 10 min
            "web_search" => 1800,        // 30 min
            "file_read" => 60,           // 1 min
            "shell" => 30,               // 30 sec (commands may have side effects)
            _ => self.ttl_secs.max(300), // Default 5 min
        }
    }

    pub fn insert(&self, tool_name: &str, arguments_json: &str, result: ToolExecutionResult) {
        let key = format!("{}:{}", tool_name, arguments_json);
        let mut cache = self.entries.lock().unwrap_or_else(|e| e.into_inner());

        if cache.len() >= self.max_entries {
            // Simple LRU: remove oldest
            let mut oldest_key = None;
            let mut oldest_time = Instant::now();

            for (k, v) in cache.iter() {
                if v.timestamp < oldest_time {
                    oldest_time = v.timestamp;
                    oldest_key = Some(k.clone());
                }
            }

            if let Some(k) = oldest_key {
                cache.remove(&k);
            }
        }

        cache.insert(
            key,
            ToolCacheEntry {
                result,
                timestamp: Instant::now(),
            },
        );
    }
}
