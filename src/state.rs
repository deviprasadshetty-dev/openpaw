use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub last_channel: Option<String>,
    pub last_chat_id: Option<String>,
    #[serde(default)]
    pub updated_at: i64,
}

pub struct StateManager {
    state_path: PathBuf,
    state: Mutex<State>,
}

impl StateManager {
    pub fn new(state_path: &str) -> Result<Self> {
        let path = PathBuf::from(state_path);
        let mgr = Self {
            state_path: path,
            state: Mutex::new(State::default()),
        };
        // Load initial state if exists
        if mgr.state_path.exists() {
            mgr.load()?;
        }
        Ok(mgr)
    }

    pub fn set_last_channel(&self, channel: &str, chat_id: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.last_channel = Some(channel.to_string());
        state.last_chat_id = Some(chat_id.to_string());
        state.updated_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
    }

    pub fn get_last_channel(&self) -> (Option<String>, Option<String>) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (state.last_channel.clone(), state.last_chat_id.clone())
    }

    pub fn save(&self) -> Result<()> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let json = serde_json::to_string_pretty(&*state)?;

        let tmp_path = self.state_path.with_extension("tmp");
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&tmp_path, json)?;
        fs::rename(tmp_path, &self.state_path)?;

        Ok(())
    }

    pub fn load(&self) -> Result<()> {
        if !self.state_path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(&self.state_path)?;
        let loaded_state: State = serde_json::from_str(&content)?;

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *state = loaded_state;

        Ok(())
    }
}

pub fn default_state_path(workspace_dir: &str) -> PathBuf {
    Path::new(workspace_dir).join("state.json")
}
