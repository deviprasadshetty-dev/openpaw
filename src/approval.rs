/// Human-in-the-loop approval system.
///
/// A subagent calls `request_approval` to pause and ask the human a yes/no question
/// before proceeding with a destructive or sensitive action.
/// The main agent (or a dedicated tool) calls `respond` with the human's decision.
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;

pub struct PendingApproval {
    pub question: String,
    pub task_label: String,
    pub created_at: u64,
    /// Sending `true` = approved, `false` = denied.
    sender: oneshot::Sender<bool>,
}

pub struct ApprovalManager {
    pending: Arc<Mutex<HashMap<u64, PendingApproval>>>,
    next_id: Arc<Mutex<u64>>,
}

impl ApprovalManager {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }

    /// Register a pending approval and return the approval_id.
    /// The caller should then await the returned `receiver`.
    pub fn register(&self, question: &str, task_label: &str) -> (u64, oneshot::Receiver<bool>) {
        let (tx, rx) = oneshot::channel();
        let mut id_guard = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
        let id = *id_guard;
        *id_guard += 1;
        drop(id_guard);

        let approval = PendingApproval {
            question: question.to_string(),
            task_label: task_label.to_string(),
            created_at: now_secs(),
            sender: tx,
        };
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, approval);
        (id, rx)
    }

    /// Respond to a pending approval. Returns error if the ID is unknown or already answered.
    pub fn respond(&self, approval_id: u64, approved: bool) -> Result<String> {
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        match guard.remove(&approval_id) {
            None => Err(anyhow!(
                "Approval ID {} not found (already answered or expired)",
                approval_id
            )),
            Some(pa) => {
                let verb = if approved { "approved" } else { "denied" };
                // Ignore send error: the waiting task may have timed out already
                let _ = pa.sender.send(approved);
                Ok(format!(
                    "Approval {} {} for: {}",
                    approval_id, verb, pa.question
                ))
            }
        }
    }

    /// List all pending approvals so the main agent can show them to the user.
    pub fn list_pending(&self) -> Vec<(u64, String, String)> {
        let guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .iter()
            .map(|(id, pa)| (*id, pa.question.clone(), pa.task_label.clone()))
            .collect()
    }

    /// Expire approvals older than `max_age_secs`, denying them automatically.
    pub fn expire_old(&self, max_age_secs: u64) {
        let now = now_secs();
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());

        // Drain expired entries separately so we can consume the sender
        let expired_ids: Vec<u64> = guard
            .iter()
            .filter(|(_, pa)| now.saturating_sub(pa.created_at) > max_age_secs)
            .map(|(id, _)| *id)
            .collect();

        for id in expired_ids {
            if let Some(pa) = guard.remove(&id) {
                let _ = pa.sender.send(false); // auto-deny expired approvals
            }
        }
    }
}

impl Default for ApprovalManager {
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
