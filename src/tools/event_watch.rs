/// Feature 1: Reactive event triggers — watch side.
///
/// Register a watcher that fires when a matching event is emitted. Supports
/// agent task prompts and shell commands as actions. Use `event_emit` to fire events.
use super::{Tool, ToolContext, ToolResult};
use crate::events::{EventRegistry, WatcherAction};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct EventWatchTool {
    pub event_registry: Arc<EventRegistry>,
}

#[async_trait]
impl Tool for EventWatchTool {
    fn name(&self) -> &str {
        "event_watch"
    }

    fn description(&self) -> &str {
        "Register a watcher that fires automatically when a matching event is emitted. \
         Patterns support exact names ('deploy.success'), prefix wildcards ('deploy.*'), \
         or '*' to match all events. \
         Actions can be an agent task prompt (the prompt is sent as a new message to the \
         main agent) or a shell command. \
         Use event_emit to trigger watchers. Use event_unwatch to remove them."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "event_pattern": {
                    "type": "string",
                    "description": "Event name pattern to watch. Supports exact match, prefix wildcard (e.g. 'file.*'), or '*' for all events."
                },
                "action_type": {
                    "type": "string",
                    "enum": ["agent_task", "shell_command"],
                    "description": "What to do when the event fires: 'agent_task' sends a prompt to the main agent; 'shell_command' runs a shell command"
                },
                "prompt": {
                    "type": "string",
                    "description": "For action_type='agent_task': the prompt to send (event name and payload will be prepended automatically)"
                },
                "command": {
                    "type": "string",
                    "description": "For action_type='shell_command': the shell command to run (EVENT_NAME and EVENT_PAYLOAD env vars are set)"
                },
                "agent_id": {
                    "type": "string",
                    "description": "For action_type='agent_task': optional named agent profile to use (from config.agents)"
                },
                "label": {
                    "type": "string",
                    "description": "Optional human-readable label for this watcher"
                },
                "unwatch_id": {
                    "type": "integer",
                    "description": "If provided, remove the watcher with this ID instead of registering a new one"
                },
                "list": {
                    "type": "boolean",
                    "description": "If true, list all registered watchers instead of creating a new one"
                }
            }
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolResult> {
        // List mode
        if args.get("list").and_then(|v| v.as_bool()).unwrap_or(false) {
            let watchers = self.event_registry.list();
            if watchers.is_empty() {
                return Ok(ToolResult::ok("No event watchers registered."));
            }
            let mut out = format!("{} watcher(s):\n\n", watchers.len());
            for w in &watchers {
                let action_desc = match &w.action {
                    WatcherAction::AgentTask { prompt, agent_id } => format!(
                        "agent_task{}:\n  {}",
                        agent_id.as_deref().map(|a| format!(" ({})", a)).unwrap_or_default(),
                        &prompt[..prompt.len().min(80)]
                    ),
                    WatcherAction::ShellCommand { command } => {
                        format!("shell: {}", &command[..command.len().min(60)])
                    }
                };
                out.push_str(&format!(
                    "• **ID {}** pattern='{}' fires={} label='{}'\n  action={}\n\n",
                    w.id, w.event_pattern, w.fire_count, w.label, action_desc
                ));
            }
            return Ok(ToolResult::ok(out.trim_end().to_string()));
        }

        // Unwatch mode
        if let Some(unwatch_id) = args.get("unwatch_id").and_then(|v| v.as_u64()) {
            if self.event_registry.unwatch(unwatch_id) {
                return Ok(ToolResult::ok(format!(
                    "Watcher {} removed.",
                    unwatch_id
                )));
            } else {
                return Ok(ToolResult::fail(format!(
                    "Watcher {} not found.",
                    unwatch_id
                )));
            }
        }

        // Register new watcher
        let event_pattern = match args.get("event_pattern").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p.trim().to_string(),
            _ => return Ok(ToolResult::fail("Missing 'event_pattern'")),
        };

        let action_type = args
            .get("action_type")
            .and_then(|v| v.as_str())
            .unwrap_or("agent_task");

        let action = match action_type {
            "agent_task" => {
                let prompt = match args.get("prompt").and_then(|v| v.as_str()) {
                    Some(p) if !p.trim().is_empty() => p.trim().to_string(),
                    _ => return Ok(ToolResult::fail("'prompt' is required for action_type='agent_task'")),
                };
                let agent_id = args
                    .get("agent_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                WatcherAction::AgentTask { prompt, agent_id }
            }
            "shell_command" => {
                let command = match args.get("command").and_then(|v| v.as_str()) {
                    Some(c) if !c.trim().is_empty() => c.trim().to_string(),
                    _ => return Ok(ToolResult::fail("'command' is required for action_type='shell_command'")),
                };
                WatcherAction::ShellCommand { command }
            }
            other => {
                return Ok(ToolResult::fail(format!(
                    "Unknown action_type '{}'. Use 'agent_task' or 'shell_command'.",
                    other
                )));
            }
        };

        let label = args
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or(&event_pattern)
            .to_string();

        let watcher_id = self.event_registry.watch(
            &event_pattern,
            action,
            &label,
            &context.channel,
            &context.chat_id,
        );

        Ok(ToolResult::ok(format!(
            "⚡ Watcher {} registered for pattern '{}' (label: '{}').\n\
             Use event_emit to trigger it, or event_watch with unwatch_id={} to remove it.",
            watcher_id, event_pattern, label, watcher_id
        )))
    }
}
