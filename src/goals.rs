use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GoalStatus {
    Todo,
    InProgress,
    Completed,
    Blocked,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub description: String,
    pub status: GoalStatus,
    pub priority: u8, // 1 (highest) to 5 (lowest)
    pub created_at: u64,
    pub updated_at: u64,
    pub progress: Option<String>,
}

pub struct GoalManager {
    pub workspace_dir: PathBuf,
    goals: Arc<Mutex<HashMap<String, Goal>>>,
}

impl GoalManager {
    pub fn new(workspace_dir: PathBuf) -> Self {
        let manager = Self {
            workspace_dir,
            goals: Arc::new(Mutex::new(HashMap::new())),
        };
        manager.load();
        manager
    }

    fn state_file(&self) -> PathBuf {
        self.workspace_dir.join("goals.json")
    }

    pub fn load(&self) {
        let path = self.state_file();
        if let Ok(content) = fs::read_to_string(path) {
            if let Ok(goals) = serde_json::from_str::<HashMap<String, Goal>>(&content) {
                let mut guard = self.goals.lock().unwrap();
                *guard = goals;
            }
        }
    }

    pub fn save(&self) {
        let path = self.state_file();
        let guard = self.goals.lock().unwrap();
        if let Ok(content) = serde_json::to_string_pretty(&*guard) {
            let _ = fs::write(path, content);
        }
    }

    pub fn add_goal(&self, description: &str, priority: u8) -> String {
        let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let goal = Goal {
            id: id.clone(),
            description: description.to_string(),
            status: GoalStatus::Todo,
            priority,
            created_at: now,
            updated_at: now,
            progress: None,
        };

        {
            let mut guard = self.goals.lock().unwrap();
            guard.insert(id.clone(), goal);
        }
        self.save();
        id
    }

    pub fn list_goals(&self) -> Vec<Goal> {
        let guard = self.goals.lock().unwrap();
        let mut list: Vec<Goal> = guard.values().cloned().collect();
        list.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then(b.created_at.cmp(&a.created_at))
        });
        list
    }

    pub fn update_goal(
        &self,
        id: &str,
        status: Option<GoalStatus>,
        progress: Option<String>,
    ) -> Result<()> {
        let mut guard = self.goals.lock().unwrap();
        if let Some(goal) = guard.get_mut(id) {
            if let Some(s) = status {
                goal.status = s;
            }
            if let Some(p) = progress {
                goal.progress = Some(p);
            }
            goal.updated_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            drop(guard);
            self.save();
            Ok(())
        } else {
            Err(anyhow!("Goal not found"))
        }
    }

    pub fn remove_goal(&self, id: &str) -> Result<()> {
        let mut guard = self.goals.lock().unwrap();
        if guard.remove(id).is_some() {
            drop(guard);
            self.save();
            Ok(())
        } else {
            Err(anyhow!("Goal not found"))
        }
    }
}
