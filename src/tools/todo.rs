//! Todo list tool — in-memory task management for the agent.
//!
//! Lets the agent break down complex tasks into subtasks, track progress,
//! and maintain focus across long conversations. One list per session.
//!
//! Actions:
//!   todo_write — create or update todo items (replace or merge)
//!   todo_read  — return the full current list
//!
//! Item fields: id (unique), content (description), status (pending|in_progress|completed|cancelled)

use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

// ── Data types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: String,
}

const VALID_STATUSES: &[&str] = &["pending", "in_progress", "completed", "cancelled"];

fn normalize_status(raw: &str) -> String {
    let s = raw.trim().to_lowercase();
    if VALID_STATUSES.contains(&s.as_str()) {
        s
    } else {
        "pending".to_string()
    }
}

// ── Todo store ────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct TodoStore {
    items: Vec<TodoItem>,
}

impl TodoStore {
    pub fn write(
        &mut self,
        todos: Vec<Value>,
        merge: bool,
    ) -> Vec<TodoItem> {
        let incoming: Vec<TodoItem> = todos
            .iter()
            .map(|v| {
                let id = v
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .trim()
                    .to_string();
                let content = v
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no description)")
                    .trim()
                    .to_string();
                let status = normalize_status(
                    v.get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("pending"),
                );
                TodoItem { id, content, status }
            })
            .collect();

        // Deduplicate by id (keep last occurrence)
        let incoming: Vec<TodoItem> = {
            let mut seen: HashMap<String, TodoItem> = HashMap::new();
            for item in incoming {
                if !item.id.is_empty() && item.id != "?" {
                    seen.insert(item.id.clone(), item);
                }
            }
            seen.into_values().collect()
        };

        if !merge {
            self.items = incoming;
        } else {
            // Merge: update existing items by id, append new ones
            let mut existing_by_id: HashMap<String, usize> = HashMap::new();
            for (i, item) in self.items.iter().enumerate() {
                existing_by_id.insert(item.id.clone(), i);
            }

            for new_item in &incoming {
                if new_item.id.is_empty() || new_item.id == "?" {
                    continue;
                }
                if let Some(&idx) = existing_by_id.get(&new_item.id) {
                    // Update only the fields the agent provided
                    if !new_item.content.is_empty()
                        && new_item.content != "(no description)"
                    {
                        self.items[idx].content = new_item.content.clone();
                    }
                    self.items[idx].status = new_item.status.clone();
                } else {
                    // New item — append
                    self.items.push(new_item.clone());
                }
            }
        }

        self.items.clone()
    }

    pub fn read(&self) -> Vec<TodoItem> {
        self.items.clone()
    }
}

// ── Tool ──────────────────────────────────────────────────────────────────

pub struct TodoTool {
    store: Arc<Mutex<TodoStore>>,
}

impl TodoTool {
    pub fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(TodoStore::default())),
        }
    }
}

impl Default for TodoTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for TodoTool {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
        }
    }
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "In-memory task list for planning and tracking progress across a conversation. \
         Use todo_write to create/update items (provide an array of {id, content, status} \
         plus optional merge flag). Use todo_read to see the full list. \
         Status: pending, in_progress, completed, cancelled. \
         Best practice: call todo_read before todo_write to see what's already there."
    }

    fn parameters_json(&self) -> String {
        r#"{
          "type": "object",
          "properties": {
            "action": {
              "type": "string",
              "enum": ["todo_write", "todo_read"],
              "description": "Action: todo_write to create/update items, todo_read to view the list"
            },
            "todos": {
              "type": "array",
              "description": "Array of todo items. Each item has: id (unique identifier string), content (task description), status (pending|in_progress|completed|cancelled). Required for todo_write.",
              "items": {
                "type": "object",
                "properties": {
                  "id": { "type": "string", "description": "Unique identifier for this task (e.g. '1', 'setup-db', 'write-tests')" },
                  "content": { "type": "string", "description": "Description of the task" },
                  "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"], "description": "Current status (default: pending)" }
                },
                "required": ["id", "content"]
              }
            },
            "merge": {
              "type": "boolean",
              "description": "If true, update existing items by id and append new ones. If false (default), replace the entire list."
            }
          },
          "required": ["action"]
        }"#
        .to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");

        match action {
            "todo_write" => {
                let todos = args
                    .get("todos")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                if todos.is_empty() {
                    return Ok(ToolResult::fail(
                        "todos array is required for todo_write. Provide at least one item with id, content, and optional status.",
                    ));
                }

                let merge = args
                    .get("merge")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let items = {
                    let mut store = self.store.lock().await;
                    store.write(todos, merge)
                };

                Ok(ToolResult::ok(format_todo_response(&items, "Updated")))
            }
            "todo_read" => {
                let items = {
                    let store = self.store.lock().await;
                    store.read()
                };
                if items.is_empty() {
                    Ok(ToolResult::ok(
                        json!({"message": "Todo list is empty. Use todo_write to add items.", "todos": []}).to_string(),
                    ))
                } else {
                    Ok(ToolResult::ok(format_todo_response(&items, "Current")))
                }
            }
            _ => Ok(ToolResult::fail(format!(
                "Unknown action '{}'. Available: todo_write, todo_read",
                action
            ))),
        }
    }
}

fn status_marker(status: &str) -> &'static str {
    match status {
        "completed" => "[x]",
        "in_progress" => "[>]",
        "pending" => "[ ]",
        "cancelled" => "[~]",
        _ => "[?]",
    }
}

fn format_todo_response(items: &[TodoItem], label: &str) -> String {
    let mut lines = vec![format!("{} todo list ({} items):", label, items.len())];
    for item in items {
        let marker = status_marker(&item.status);
        lines.push(format!(
            "  {} {} - {} [{}]",
            marker, item.id, item.content, item.status
        ));
    }
    lines.join("\n")
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(id: &str, content: &str, status: &str) -> Value {
        json!({"id": id, "content": content, "status": status})
    }

    #[test]
    fn test_write_replace() {
        let mut store = TodoStore::default();
        let items = store.write(
            vec![make_item("1", "Do thing", "pending")],
            false,
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "1");

        // Replace
        let items = store.write(
            vec![
                make_item("2", "Different", "in_progress"),
                make_item("3", "Another", "pending"),
            ],
            false,
        );
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "2");
    }

    #[test]
    fn test_write_merge() {
        let mut store = TodoStore::default();
        store.write(
            vec![
                make_item("1", "Task A", "pending"),
                make_item("2", "Task B", "pending"),
            ],
            false,
        );

        // Merge: update item 1, add item 3
        let items = store.write(
            vec![
                make_item("1", "Task A", "completed"),
                make_item("3", "Task C", "pending"),
            ],
            true,
        );

        assert_eq!(items.len(), 3);
        assert_eq!(items[0].status, "completed"); // Item 1 updated
        assert_eq!(items[2].id, "3"); // Item 3 appended
    }

    #[test]
    fn test_normalize_status() {
        assert_eq!(normalize_status("pending"), "pending");
        assert_eq!(normalize_status("IN_PROGRESS"), "in_progress");
        assert_eq!(normalize_status("Completed"), "completed");
        assert_eq!(normalize_status("cancelled"), "cancelled");
        assert_eq!(normalize_status("unknown"), "pending");
        assert_eq!(normalize_status(""), "pending");
    }

    #[test]
    fn test_read_empty() {
        let store = TodoStore::default();
        assert!(store.read().is_empty());
    }
}
