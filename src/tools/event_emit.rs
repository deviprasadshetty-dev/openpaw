/// Feature 1: Reactive event triggers — emit side.
///
/// Emit a named event with an optional payload. Any registered watchers
/// (via `event_watch`) whose pattern matches the event name will be triggered.
use super::{Tool, ToolContext, ToolResult};
use crate::events::EventRegistry;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct EventEmitTool {
    pub event_registry: Arc<EventRegistry>,
}

#[async_trait]
impl Tool for EventEmitTool {
    fn name(&self) -> &str {
        "event_emit"
    }

    fn description(&self) -> &str {
        "Emit a named event, triggering any registered watchers whose pattern matches. \
         Use dot-notation for event namespacing (e.g. 'file.changed', 'deploy.success', \
         'memory.recall.found'). Returns the number of watchers triggered."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "event": {
                    "type": "string",
                    "description": "Event name in dot-notation (e.g. \"file.changed\", \"task.completed\", \"webhook.received\")"
                },
                "payload": {
                    "type": "string",
                    "description": "Optional event payload (data passed to watchers, e.g. file path, JSON, message)"
                }
            },
            "required": ["event"]
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolResult> {
        let event = match args.get("event").and_then(|v| v.as_str()) {
            Some(e) if !e.trim().is_empty() => e.trim().to_string(),
            _ => return Ok(ToolResult::fail("Missing or empty 'event' parameter")),
        };

        let payload = args
            .get("payload")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let triggered =
            self.event_registry
                .emit(&event, &payload, &context.channel, &context.chat_id);

        if triggered == 0 {
            Ok(ToolResult::ok(format!(
                "⚡ Event '{}' emitted — no watchers matched.",
                event
            )))
        } else {
            Ok(ToolResult::ok(format!(
                "⚡ Event '{}' emitted — {} watcher(s) triggered.",
                event, triggered
            )))
        }
    }
}
