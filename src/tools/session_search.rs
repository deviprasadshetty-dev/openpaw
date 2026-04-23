use crate::providers::{ChatMessage, ChatRequest, Provider};
use crate::tools::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use rusqlite::params;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

pub struct SessionSearchTool {
    pub workspace_dir: String,
    pub provider: Option<Arc<dyn Provider>>,
    pub model_name: String,
}

#[async_trait]
impl Tool for SessionSearchTool {
    fn name(&self) -> &str {
        "session_search"
    }

    fn description(&self) -> &str {
        "Search across past conversation sessions using full-text search. \
         Query your episodic memory to find how you solved similar problems, \
         what the user preferred, or what happened in previous sessions. \
         Loads full session context around matches and returns a concise summary."
    }

    fn parameters_json(&self) -> String {
        r#"{
  "type": "object",
  "properties": {
    "query": {
      "type": "string",
      "description": "Search query (supports FTS5 syntax: plain words, OR, AND, phrase quotes)"
    },
    "role_filter": {
      "type": "string",
      "description": "Optional: filter by role (user, assistant, tool). Comma-separated for multiple."
    },
    "limit": {
      "type": "integer",
      "description": "Maximum number of sessions to return (default: 3)",
      "default": 3
    }
  },
  "required": ["query"]
}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim(),
            _ => return Ok(ToolResult::fail("Missing or empty 'query' parameter")),
        };

        let role_filter = args.get("role_filter").and_then(|v| v.as_str());
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(3) as usize;

        let db_path = format!("{}/memory.db", self.workspace_dir);
        if !Path::new(&db_path).exists() {
            return Ok(ToolResult::fail(
                "No SQLite memory database found. Session search requires the sqlite memory backend."
            ));
        }

        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::fail(format!("Failed to open database: {}", e))),
        };

        let fts_exists: bool = match conn.query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='messages_fts'",
            [],
            |_| Ok(true),
        ) {
            Ok(true) => true,
            _ => false,
        };

        let session_ids = if fts_exists {
            match self.find_matching_sessions(&conn, query, role_filter, limit) {
                Ok(ids) => ids,
                Err(e) => return Ok(ToolResult::fail(format!("Search failed: {}", e))),
            }
        } else {
            match self.find_matching_sessions_fallback(&conn, query, role_filter, limit) {
                Ok(ids) => ids,
                Err(e) => return Ok(ToolResult::fail(format!("Search failed: {}", e))),
            }
        };

        if session_ids.is_empty() {
            return Ok(ToolResult::ok("No matching sessions found.".to_string()));
        }

        // Load full session context for each match
        let mut session_contexts = Vec::new();
        for sid in &session_ids {
            match self.load_session_context(&conn, sid) {
                Ok(ctx) => session_contexts.push((sid.clone(), ctx)),
                Err(_) => {}
            }
        }

        // If provider available, summarize each session
        if let Some(ref provider) = self.provider {
            let summaries = self.summarize_sessions(provider, &session_contexts).await;
            Ok(ToolResult::ok(summaries))
        } else {
            // Fallback: return raw truncated context
            let raw = self.format_raw_context(&session_contexts);
            Ok(ToolResult::ok(raw))
        }
    }
}

impl SessionSearchTool {
    fn find_matching_sessions(
        &self,
        conn: &rusqlite::Connection,
        query: &str,
        role_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        let fts_query: String = query
            .split_whitespace()
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");

        let sql = if let Some(roles) = role_filter {
            let role_list: Vec<String> = roles.split(',').map(|s| s.trim().to_string()).collect();
            let placeholders = role_list.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            format!(
                "SELECT DISTINCT m.session_id \
                 FROM messages_fts f \
                 JOIN messages m ON m.rowid = f.rowid \
                 WHERE messages_fts MATCH ? AND m.role IN ({}) \
                 ORDER BY bm25(messages_fts) \
                 LIMIT ?",
                placeholders
            )
        } else {
            "SELECT DISTINCT m.session_id \
             FROM messages_fts f \
             JOIN messages m ON m.rowid = f.rowid \
             WHERE messages_fts MATCH ? \
             ORDER BY bm25(messages_fts) \
             LIMIT ?"
            .to_string()
        };

        let mut stmt = conn.prepare(&sql)?;
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        params_vec.push(Box::new(fts_query));
        if let Some(roles) = role_filter {
            for r in roles.split(',').map(|s| s.trim()) {
                params_vec.push(Box::new(r.to_string()));
            }
        }
        params_vec.push(Box::new(limit as i64));

        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
            row.get::<_, String>(0)
        })?;

        let mut ids = Vec::new();
        for r in rows {
            ids.push(r?);
        }
        Ok(ids)
    }

    fn find_matching_sessions_fallback(
        &self,
        conn: &rusqlite::Connection,
        query: &str,
        role_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        let terms: Vec<&str> = query.split_whitespace().collect();
        let mut sql = "SELECT DISTINCT session_id FROM messages WHERE 1=1".to_string();

        if !terms.is_empty() {
            sql.push_str(" AND (");
            for (i, _) in terms.iter().enumerate() {
                if i > 0 {
                    sql.push_str(" OR ");
                }
                sql.push_str(&format!("content LIKE ?{} ESCAPE '\\\\'", i + 1));
            }
            sql.push(')');
        }

        if let Some(roles) = role_filter {
            let role_list: Vec<String> = roles.split(',').map(|s| s.trim().to_string()).collect();
            let placeholders = role_list.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            sql.push_str(&format!(" AND role IN ({}) ", placeholders));
        }

        sql.push_str(" ORDER BY id DESC LIMIT ?");

        let mut stmt = conn.prepare(&sql)?;
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        for t in &terms {
            let like_term = format!("%{}%", t.replace('%', "\\\\%").replace('_', "\\\\_"));
            params_vec.push(Box::new(like_term));
        }
        if let Some(roles) = role_filter {
            for r in roles.split(',').map(|s| s.trim()) {
                params_vec.push(Box::new(r.to_string()));
            }
        }
        params_vec.push(Box::new(limit as i64));

        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
            row.get::<_, String>(0)
        })?;

        let mut ids = Vec::new();
        for r in rows {
            ids.push(r?);
        }
        Ok(ids)
    }

    fn load_session_context(&self, conn: &rusqlite::Connection, session_id: &str) -> Result<String> {
        let mut stmt = conn.prepare(
            "SELECT role, content, created_at FROM messages WHERE session_id = ? ORDER BY id ASC"
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;

        let mut parts = Vec::new();
        for row in rows {
            let (role, content, created_at) = row?;
            let ts = created_at.unwrap_or_else(|| "unknown".to_string());
            parts.push(format!("[{} @ {}]: {}", role, ts, content));
        }

        Ok(parts.join("\n"))
    }

    fn format_raw_context(
        &self,
        sessions: &[(String, String)],
    ) -> String {
        let mut result = String::from("## Past Session Results\n\n");
        for (session_id, context) in sessions {
            result.push_str(&format!("### Session: {}\n", session_id));
            let truncated = if context.len() > 800 {
                format!("{}... [truncated]", &context[..800])
            } else {
                context.clone()
            };
            result.push_str(&truncated);
            result.push_str("\n\n");
        }
        result
    }

    async fn summarize_sessions(
        &self,
        provider: &Arc<dyn Provider>,
        sessions: &[(String, String)],
    ) -> String {
        let mut summaries = Vec::new();
        for (session_id, context) in sessions {
            // Truncate context to ~15k chars to stay within token limits
            let truncated = if context.len() > 15000 {
                format!("{}... [truncated]", &context[..15000])
            } else {
                context.clone()
            };

            let prompt = format!(
                "You are summarizing a past conversation session for retrieval. \
                 Extract: what the user wanted, what approach was taken, what worked or failed, \
                 and any key preferences or corrections. Be concise (3-5 sentences).\n\n\
                 Session:\n{}\n\nSummary:",
                truncated
            );

            let request = ChatRequest {
                messages: &[ChatMessage::user(prompt)],
                model: &self.model_name,
                temperature: 0.3,
                max_tokens: Some(400),
                tools: None,
                timeout_secs: 30,
                reasoning_effort: None,
            };

            let summary = match provider.chat(&request) {
                Ok(resp) => resp.content.unwrap_or_else(|| "(no summary)".to_string()),
                Err(_) => "(summary failed)".to_string(),
            };

            summaries.push(format!(
                "### Session: {}\n{}",
                session_id, summary
            ));
        }

        summaries.join("\n\n")
    }
}
