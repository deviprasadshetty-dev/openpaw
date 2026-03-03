use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationAction {
    Set,
    Unset,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MutationResult {
    pub path: String,
    pub changed: bool,
    pub applied: bool,
    pub requires_restart: bool,
    pub old_value_json: String,
    pub new_value_json: String,
    pub backup_path: Option<String>,
}

pub fn mutate_config(_action: MutationAction, path: &str, _value: Option<&str>) -> Result<MutationResult> {
    // Stub implementation
    Ok(MutationResult {
        path: path.to_string(),
        changed: false,
        applied: false,
        requires_restart: false,
        old_value_json: "null".to_string(),
        new_value_json: "null".to_string(),
        backup_path: None,
    })
}
