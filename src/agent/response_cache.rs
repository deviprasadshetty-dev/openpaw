use crate::providers::ChatMessage;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct ResponseCacheEntry {
    pub response: String,
    pub timestamp: Instant,
}

pub struct ResponseCache {
    entries: Arc<Mutex<HashMap<String, ResponseCacheEntry>>>,
    ttl_secs: u64,
}

impl ResponseCache {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            ttl_secs,
        }
    }

    fn compute_key(messages: &[ChatMessage], model: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(model.as_bytes());
        for msg in messages {
            hasher.update(msg.role.as_bytes());
            hasher.update(msg.content.as_bytes());
            if let Some(ref name) = msg.name {
                hasher.update(name.as_bytes());
            }
        }
        format!("{:x}", hasher.finalize())
    }

    pub fn get(&self, messages: &[ChatMessage], model: &str) -> Option<String> {
        let key = Self::compute_key(messages, model);
        let mut cache = self.entries.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(entry) = cache.get(&key) {
            if entry.timestamp.elapsed() < Duration::from_secs(self.ttl_secs) {
                return Some(entry.response.clone());
            } else {
                cache.remove(&key);
            }
        }
        None
    }

    pub fn insert(&self, messages: &[ChatMessage], model: &str, response: String) {
        let key = Self::compute_key(messages, model);
        let mut cache = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(
            key,
            ResponseCacheEntry {
                response,
                timestamp: Instant::now(),
            },
        );
    }
}

impl Clone for ResponseCache {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
            ttl_secs: self.ttl_secs,
        }
    }
}
