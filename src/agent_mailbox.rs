/// Shared in-memory mailbox for inter-agent messaging.
///
/// Subagents can post messages to named mailboxes; other agents (or the main agent)
/// can read from those mailboxes. Messages are consumed in FIFO order.
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct MailboxMessage {
    pub from: String,
    pub content: String,
    pub sent_at: u64,
}

pub struct AgentMailbox {
    boxes: Arc<Mutex<HashMap<String, VecDeque<MailboxMessage>>>>,
}

impl AgentMailbox {
    pub fn new() -> Self {
        Self {
            boxes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Post a message to a named mailbox.
    pub fn post(&self, mailbox: &str, from: &str, content: &str) {
        let msg = MailboxMessage {
            from: from.to_string(),
            content: content.to_string(),
            sent_at: now_secs(),
        };
        let mut guard = self.boxes.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .entry(mailbox.to_string())
            .or_insert_with(VecDeque::new)
            .push_back(msg);
    }

    /// Receive up to `limit` messages from a mailbox (consuming them).
    /// Returns messages oldest-first.
    pub fn recv(&self, mailbox: &str, limit: usize) -> Vec<MailboxMessage> {
        let mut guard = self.boxes.lock().unwrap_or_else(|e| e.into_inner());
        let queue = match guard.get_mut(mailbox) {
            Some(q) => q,
            None => return Vec::new(),
        };
        let take = limit.min(queue.len());
        queue.drain(..take).collect()
    }

    /// Peek at messages without consuming them.
    pub fn peek(&self, mailbox: &str, limit: usize) -> Vec<MailboxMessage> {
        let guard = self.boxes.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get(mailbox) {
            Some(q) => q.iter().take(limit).cloned().collect(),
            None => Vec::new(),
        }
    }

    /// Count pending messages in a mailbox.
    pub fn count(&self, mailbox: &str) -> usize {
        let guard = self.boxes.lock().unwrap_or_else(|e| e.into_inner());
        guard.get(mailbox).map(|q| q.len()).unwrap_or(0)
    }

    /// List all mailboxes that have at least one message.
    pub fn list_nonempty(&self) -> Vec<(String, usize)> {
        let guard = self.boxes.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .iter()
            .filter(|(_, q)| !q.is_empty())
            .map(|(name, q)| (name.clone(), q.len()))
            .collect()
    }
}

impl Default for AgentMailbox {
    fn default() -> Self {
        Self::new()
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
