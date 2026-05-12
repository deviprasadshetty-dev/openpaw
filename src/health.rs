use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ComponentStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    pub name: String,
    pub status: ComponentStatus,
    pub last_error: Option<String>,
    pub error_count: u32,
    pub last_check_secs: u64,
}

struct HealthRegistry {
    components: HashMap<String, ComponentHealth>,
}

impl HealthRegistry {
    fn new() -> Self {
        Self {
            components: HashMap::new(),
        }
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn mark_error(&mut self, name: &str, error: &str) {
        let entry = self
            .components
            .entry(name.to_string())
            .or_insert_with(|| ComponentHealth {
                name: name.to_string(),
                status: ComponentStatus::Healthy,
                last_error: None,
                error_count: 0,
                last_check_secs: Self::now(),
            });

        entry.error_count += 1;
        entry.last_error = Some(error.to_string());
        entry.last_check_secs = Self::now();

        if entry.error_count >= 5 {
            entry.status = ComponentStatus::Unhealthy;
        } else if entry.error_count >= 2 {
            entry.status = ComponentStatus::Degraded;
        }
    }

    fn mark_ok(&mut self, name: &str) {
        let entry = self
            .components
            .entry(name.to_string())
            .or_insert_with(|| ComponentHealth {
                name: name.to_string(),
                status: ComponentStatus::Healthy,
                last_error: None,
                error_count: 0,
                last_check_secs: Self::now(),
            });

        entry.error_count = 0;
        entry.last_error = None;
        entry.status = ComponentStatus::Healthy;
        entry.last_check_secs = Self::now();
    }
}

fn get_registry() -> Arc<Mutex<HealthRegistry>> {
    static REGISTRY: OnceLock<Arc<Mutex<HealthRegistry>>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| Arc::new(Mutex::new(HealthRegistry::new())))
        .clone()
}

pub fn mark_component_error(name: &str, error: &str) {
    if let Ok(mut reg) = get_registry().lock() {
        reg.mark_error(name, error);
    }
    tracing::warn!("Component {} error: {}", name, error);
}

pub fn mark_component_ok(name: &str) {
    if let Ok(mut reg) = get_registry().lock() {
        reg.mark_ok(name);
    }
}

pub fn get_health_status() -> Vec<ComponentHealth> {
    if let Ok(reg) = get_registry().lock() {
        reg.components.values().cloned().collect()
    } else {
        Vec::new()
    }
}
