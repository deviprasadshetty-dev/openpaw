use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Chat,
    ToolUse,
    Summarize,
    Greeting,
    Cron,
    Subagent,
    Heartbeat,
    Event,
}

impl TaskKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskKind::Chat => "chat",
            TaskKind::ToolUse => "tool_use",
            TaskKind::Summarize => "summarize",
            TaskKind::Greeting => "greeting",
            TaskKind::Cron => "cron",
            TaskKind::Subagent => "subagent",
            TaskKind::Heartbeat => "heartbeat",
            TaskKind::Event => "event",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "chat" => Some(TaskKind::Chat),
            "tool_use" => Some(TaskKind::ToolUse),
            "summarize" => Some(TaskKind::Summarize),
            "greeting" => Some(TaskKind::Greeting),
            "cron" => Some(TaskKind::Cron),
            "subagent" => Some(TaskKind::Subagent),
            "heartbeat" => Some(TaskKind::Heartbeat),
            "event" => Some(TaskKind::Event),
            _ => None,
        }
    }
}

const ALL_TASK_KEYS: &[&str] = &[
    "chat",
    "tool_use",
    "summarize",
    "greeting",
    "cron",
    "subagent",
    "heartbeat",
    "event",
];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TaskModelConfig {
    #[serde(default)]
    pub models: HashMap<String, String>,
}

impl TaskModelConfig {
    pub fn new(default_model: &str) -> Self {
        let mut models = HashMap::new();
        let default = default_model.to_string();
        for key in ALL_TASK_KEYS {
            models.insert(key.to_string(), default.clone());
        }
        Self { models }
    }

    pub fn with_overrides(default_model: &str, overrides: &HashMap<String, String>) -> Self {
        let mut config = Self::new(default_model);
        for (task, model) in overrides {
            let normalized = task.to_lowercase().replace('-', "_");
            if config.models.contains_key(&normalized) {
                config.models.insert(normalized, model.clone());
            }
        }
        config
    }

    pub fn get(&self, task: &TaskKind) -> &str {
        self.models
            .get(task.as_str())
            .or_else(|| self.models.get("chat"))
            .map(|s| s.as_str())
            .unwrap_or("gpt-4o")
    }

    pub fn default_model(&self) -> &str {
        self.models
            .get("chat")
            .map(|s| s.as_str())
            .unwrap_or("gpt-4o")
    }
}

const GREETINGS: &[&str] = &[
    "hi",
    "hello",
    "hey",
    "hiya",
    "howdy",
    "yo",
    "sup",
    "good morning",
    "good afternoon",
    "good evening",
    "good night",
    "greetings",
    "what's up",
    "whats up",
];

pub fn is_greeting(msg: &str) -> bool {
    let lower = msg.to_lowercase().trim().to_string();
    if lower.len() > 25 {
        return false;
    }
    GREETINGS
        .iter()
        .any(|&g| lower == g || lower.starts_with(g))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_model_config_new() {
        let config = TaskModelConfig::new("gpt-4o");
        assert_eq!(config.get(&TaskKind::Chat), "gpt-4o");
        assert_eq!(config.get(&TaskKind::ToolUse), "gpt-4o");
        assert_eq!(config.get(&TaskKind::Summarize), "gpt-4o");
        assert_eq!(config.get(&TaskKind::Greeting), "gpt-4o");
        assert_eq!(config.get(&TaskKind::Cron), "gpt-4o");
        assert_eq!(config.get(&TaskKind::Subagent), "gpt-4o");
        assert_eq!(config.get(&TaskKind::Heartbeat), "gpt-4o");
        assert_eq!(config.get(&TaskKind::Event), "gpt-4o");
    }

    #[test]
    fn test_task_model_config_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert("greeting".to_string(), "gemini-2.0-flash".to_string());
        overrides.insert("summarize".to_string(), "gpt-4o-mini".to_string());
        overrides.insert("cron".to_string(), "gemini-2.0-flash".to_string());
        overrides.insert("heartbeat".to_string(), "gemini-2.0-flash".to_string());
        overrides.insert("event".to_string(), "gemini-2.0-flash".to_string());
        overrides.insert("subagent".to_string(), "gpt-4o-mini".to_string());

        let config = TaskModelConfig::with_overrides("gpt-4o", &overrides);
        assert_eq!(config.get(&TaskKind::Chat), "gpt-4o");
        assert_eq!(config.get(&TaskKind::ToolUse), "gpt-4o");
        assert_eq!(config.get(&TaskKind::Summarize), "gpt-4o-mini");
        assert_eq!(config.get(&TaskKind::Greeting), "gemini-2.0-flash");
        assert_eq!(config.get(&TaskKind::Cron), "gemini-2.0-flash");
        assert_eq!(config.get(&TaskKind::Heartbeat), "gemini-2.0-flash");
        assert_eq!(config.get(&TaskKind::Event), "gemini-2.0-flash");
        assert_eq!(config.get(&TaskKind::Subagent), "gpt-4o-mini");
    }

    #[test]
    fn test_task_kind_from_str() {
        assert_eq!(TaskKind::from_str("cron"), Some(TaskKind::Cron));
        assert_eq!(TaskKind::from_str("heartbeat"), Some(TaskKind::Heartbeat));
        assert_eq!(TaskKind::from_str("event"), Some(TaskKind::Event));
        assert_eq!(TaskKind::from_str("subagent"), Some(TaskKind::Subagent));
        assert_eq!(TaskKind::from_str("chat"), Some(TaskKind::Chat));
        assert_eq!(TaskKind::from_str("unknown"), None);
    }

    #[test]
    fn test_is_greeting() {
        assert!(is_greeting("hi"));
        assert!(is_greeting("Hello!"));
        assert!(is_greeting("hey there"));
        assert!(!is_greeting("can you help me debug this code?"));
        assert!(!is_greeting("what is the meaning of life"));
    }
}
