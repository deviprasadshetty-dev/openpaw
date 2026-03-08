use super::{MemoryCategory, MemoryEntry, MemoryStore, MessageEntry, SessionStore};
use anyhow::Result;
use rusqlite::{Connection, params};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct SqliteMemory {
    conn: Mutex<Connection>,
    embedder: Option<Arc<dyn super::embeddings::EmbeddingProvider>>,
}

impl SqliteMemory {
    pub fn new(db_path: &str) -> Result<Self> {
        let conn = Connection::open(db_path)?;

        // Pragmas
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;",
        )?;

        // Schema
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS memories (
              id         TEXT PRIMARY KEY,
              key        TEXT NOT NULL UNIQUE,
              content    TEXT NOT NULL,
              category   TEXT NOT NULL DEFAULT 'core',
              session_id TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_memories_category ON memories(category);
            CREATE INDEX IF NOT EXISTS idx_memories_key ON memories(key);
            CREATE INDEX IF NOT EXISTS idx_memories_session ON memories(session_id);

            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
              key, content, content=memories, content_rowid=rowid
            );

            CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
              INSERT INTO memories_fts(rowid, key, content)
              VALUES (new.rowid, new.key, new.content);
            END;
            CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
              INSERT INTO memories_fts(memories_fts, rowid, key, content)
              VALUES ('delete', old.rowid, old.key, old.content);
            END;
            CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
              INSERT INTO memories_fts(memories_fts, rowid, key, content)
              VALUES ('delete', old.rowid, old.key, old.content);
              INSERT INTO memories_fts(rowid, key, content)
              VALUES (new.rowid, new.key, new.content);
            END;

            CREATE TABLE IF NOT EXISTS messages (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              session_id TEXT NOT NULL,
              role TEXT NOT NULL,
              content TEXT NOT NULL,
              created_at TEXT DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS sessions (
              id TEXT PRIMARY KEY,
              provider TEXT,
              model TEXT,
              created_at TEXT DEFAULT (datetime('now')),
              updated_at TEXT DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS kv (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );
            "#,
        )?;

        // Session ID Migration (Ignore duplicate column)
        let _ = conn.execute("ALTER TABLE memories ADD COLUMN session_id TEXT;", []);
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_memories_session ON memories(session_id);",
            [],
        );

        // Importance and Embeddings Schema
        let _ = conn.execute(
            "ALTER TABLE memories ADD COLUMN importance REAL DEFAULT 0.5;",
            [],
        );

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS embeddings (
              memory_id TEXT PRIMARY KEY,
              embedding BLOB NOT NULL,
              dim INTEGER NOT NULL
            );",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
            embedder: None,
        })
    }

    pub fn with_embedder(
        mut self,
        embedder: Arc<dyn super::embeddings::EmbeddingProvider>,
    ) -> Self {
        self.embedder = Some(embedder);
        self
    }

    pub fn store_embedding_by_key(&self, key: &str, embedding: &[f32]) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let memory_id: String = conn.query_row(
            "SELECT id FROM memories WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )?;

        let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        let dim = embedding.len() as i64;
        conn.execute(
            "INSERT INTO embeddings (memory_id, embedding, dim) VALUES (?1, ?2, ?3)
             ON CONFLICT(memory_id) DO UPDATE SET embedding = excluded.embedding, dim = excluded.dim",
            params![memory_id, blob, dim],
        )?;
        Ok(())
    }

    fn bytes_to_f32_vec(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect()
    }

    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        dot / (norm_a * norm_b)
    }

    fn now_str() -> String {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string()
    }

    fn nano_id() -> String {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let rand_hi: u64 = rand::random();
        format!("{}-{:x}", ts, rand_hi)
    }
    fn store_embedding_internal(&self, memory_id: &str, embedding: &[f32]) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        let dim = embedding.len() as i64;
        conn.execute(
            "INSERT INTO embeddings (memory_id, embedding, dim) VALUES (?1, ?2, ?3)
             ON CONFLICT(memory_id) DO UPDATE SET embedding = excluded.embedding, dim = excluded.dim",
            params![memory_id, blob, dim],
        )?;
        Ok(())
    }
}

impl MemoryStore for SqliteMemory {
    fn name(&self) -> &str {
        "sqlite"
    }

    fn store(
        &self,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
        importance: Option<f64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = Self::now_str();
        let id = Self::nano_id();
        let cat_str = category.to_string();
        let imp = importance.unwrap_or(0.5);

        conn.execute(
            "INSERT INTO memories (id, key, content, category, session_id, created_at, updated_at, importance)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(key) DO UPDATE SET
             content = excluded.content,
             category = excluded.category,
             session_id = excluded.session_id,
             updated_at = excluded.updated_at,
             importance = excluded.importance",
            params![id.clone(), key, content, cat_str, session_id, now, now, imp],
        )?;

        // Auto-embed if provider is present and not an ephemeral autosave
        if let Some(ref embedder) = self.embedder {
            if !key.starts_with("autosave_") {
                if let Ok(emb) = embedder.embed(content) {
                    let _ = self.store_embedding_internal(&id, &emb);
                }
            }
        }

        Ok(())
    }

    fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        // FTS Match
        let fts_query: Vec<String> = trimmed
            .split_whitespace()
            .map(|w| format!("\"{}\"", w.replace("\"", "\"\"")))
            .collect();
        let fts_match = fts_query.join(" OR ");

        let mut stmt = conn.prepare(
            "SELECT m.id, m.key, m.content, m.category, m.created_at, bm25(memories_fts) as score, m.session_id
             FROM memories_fts f
             JOIN memories m ON m.rowid = f.rowid
             WHERE memories_fts MATCH ?1
             ORDER BY score
             LIMIT ?2"
        )?;

        let entries = stmt.query_map(params![fts_match, limit as i64], |row| {
            let cat_str: String = row.get(3)?;
            Ok(MemoryEntry {
                id: row.get(0)?,
                key: row.get(1)?,
                content: row.get(2)?,
                category: MemoryCategory::from_str(&cat_str),
                timestamp: row.get(4)?,
                score: Some(-row.get::<_, f64>(5)?), // BM25 is negative
                session_id: row.get(6)?,
                importance: 0.5,
                embedding: None,
            })
        })?;

        let mut results = Vec::new();
        for e in entries {
            let entry = e?;
            if let Some(sid) = session_id {
                if entry.session_id.as_deref() != Some(sid) {
                    continue;
                }
            }
            results.push(entry);
        }

        if !results.is_empty() {
            return Ok(results);
        }

        // Fallback LIKE search
        let mut sql = "SELECT id, key, content, category, created_at, session_id, importance FROM memories WHERE 1=1".to_string();
        let terms: Vec<&str> = trimmed.split_whitespace().collect();

        if !terms.is_empty() {
            sql.push_str(" AND (");
            for (i, _) in terms.iter().enumerate() {
                if i > 0 {
                    sql.push_str(" OR ");
                }
                sql.push_str(&format!(
                    "(content LIKE ?{} ESCAPE '\\' OR key LIKE ?{} ESCAPE '\\')",
                    i * 2 + 1,
                    i * 2 + 2
                ));
            }
            sql.push_str(")");
        }

        if let Some(_sid) = session_id {
            sql.push_str(&format!(" AND session_id = ?{}", terms.len() * 2 + 1));
        }

        sql.push_str(&format!(
            " ORDER BY importance DESC, updated_at DESC LIMIT ?{}",
            terms.len() * 2 + (if session_id.is_some() { 2 } else { 1 })
        ));

        let mut stmt = conn.prepare(&sql)?;
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        for t in &terms {
            let like_term = format!("%{}%", t.replace("%", "\\%").replace("_", "\\_"));
            params_vec.push(Box::new(like_term.clone()));
            params_vec.push(Box::new(like_term));
        }
        if let Some(sid) = session_id {
            params_vec.push(Box::new(sid.to_string()));
        }
        params_vec.push(Box::new(limit as i64));

        let mut rows = stmt.query(rusqlite::params_from_iter(params_vec.iter()))?;
        while let Some(row) = rows.next()? {
            let cat_str: String = row.get(3)?;
            results.push(MemoryEntry {
                id: row.get(0)?,
                key: row.get(1)?,
                content: row.get(2)?,
                category: MemoryCategory::from_str(&cat_str),
                timestamp: row.get(4)?,
                session_id: row.get(5)?,
                score: None,
                importance: row.get(6)?,
                embedding: None,
            });
        }

        Ok(results)
    }

    fn get(&self, key: &str) -> Result<Option<MemoryEntry>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare("SELECT id, key, content, category, created_at, session_id FROM memories WHERE key = ?1")?;

        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            let cat_str: String = row.get(3)?;
            Ok(Some(MemoryEntry {
                id: row.get(0)?,
                key: row.get(1)?,
                content: row.get(2)?,
                category: MemoryCategory::from_str(&cat_str),
                timestamp: row.get(4)?,
                session_id: row.get(5)?,
                score: None,
                importance: 0.5,
                embedding: None,
            }))
        } else {
            Ok(None)
        }
    }

    fn list(
        &self,
        category: Option<MemoryCategory>,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut results = Vec::new();

        if let Some(cat) = category {
            let cat_str = cat.to_string();
            let mut stmt = conn.prepare("SELECT id, key, content, category, created_at, session_id FROM memories WHERE category = ?1 ORDER BY updated_at DESC")?;
            let mapped = stmt.query_map(params![cat_str], |row| {
                let cat_s: String = row.get(3)?;
                Ok(MemoryEntry {
                    id: row.get(0)?,
                    key: row.get(1)?,
                    content: row.get(2)?,
                    category: MemoryCategory::from_str(&cat_s),
                    timestamp: row.get(4)?,
                    session_id: row.get(5)?,
                    score: None,
                    importance: 0.5,
                    embedding: None,
                })
            })?;
            for e in mapped {
                let entry = e?;
                if let Some(sid) = session_id {
                    if entry.session_id.as_deref() != Some(sid) {
                        continue;
                    }
                }
                results.push(entry);
            }
        } else {
            let mut stmt = conn.prepare("SELECT id, key, content, category, created_at, session_id FROM memories ORDER BY updated_at DESC")?;
            let mapped = stmt.query_map([], |row| {
                let cat_s: String = row.get(3)?;
                Ok(MemoryEntry {
                    id: row.get(0)?,
                    key: row.get(1)?,
                    content: row.get(2)?,
                    category: MemoryCategory::from_str(&cat_s),
                    timestamp: row.get(4)?,
                    session_id: row.get(5)?,
                    score: None,
                    importance: 0.5,
                    embedding: None,
                })
            })?;
            for e in mapped {
                let entry = e?;
                if let Some(sid) = session_id {
                    if entry.session_id.as_deref() != Some(sid) {
                        continue;
                    }
                }
                results.push(entry);
            }
        }

        Ok(results)
    }

    fn forget(&self, key: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let deleted = conn.execute("DELETE FROM memories WHERE key = ?1", params![key])?;
        Ok(deleted > 0)
    }

    fn count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))?;
        Ok(count as usize)
    }

    fn health_check(&self) -> bool {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row("SELECT 1", [], |r| r.get::<_, i32>(0))
            .is_ok()
    }

    fn semantic_recall_by_text(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        if let Some(embedder) = &self.embedder {
            let embedding = embedder.embed(query)?;
            self.semantic_recall(&embedding, limit)
        } else {
            Ok(Vec::new())
        }
    }

    fn semantic_recall(&self, query_embedding: &[f32], limit: usize) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn.prepare("SELECT memory_id, embedding FROM embeddings")?;
        let mut rows = stmt.query([])?;

        let mut scored_ids: Vec<(String, f32)> = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            let emb = Self::bytes_to_f32_vec(&blob);
            let sim = Self::cosine_similarity(query_embedding, &emb);
            scored_ids.push((id, sim));
        }

        scored_ids.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored_ids.truncate(limit);

        if scored_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();
        for (id, sim) in scored_ids {
            let mut stmt = conn.prepare(
                "SELECT key, content, category, created_at, session_id, importance FROM memories WHERE id = ?1"
            )?;
            let mut mem_rows = stmt.query(params![id])?;
            if let Some(row) = mem_rows.next()? {
                let cat_str: String = row.get(2)?;
                results.push(MemoryEntry {
                    id: id.clone(),
                    key: row.get(0)?,
                    content: row.get(1)?,
                    category: MemoryCategory::from_str(&cat_str),
                    timestamp: row.get(3)?,
                    session_id: row.get(4)?,
                    score: Some(sim as f64),
                    importance: row.get(5)?,
                    embedding: None,
                });
            }
        }

        Ok(results)
    }

    fn decay_importance(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        // Decay importance by 10% for memories older than 1 day
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        conn.execute(
            "UPDATE memories SET importance = importance * 0.9 WHERE ?1 - CAST(updated_at AS INTEGER) > 86400",
            params![now],
        )?;
        Ok(())
    }
}

impl SessionStore for SqliteMemory {
    fn save_message(&self, session_id: &str, role: &str, content: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO messages (session_id, role, content) VALUES (?, ?, ?)",
            params![session_id, role, content],
        )?;
        Ok(())
    }

    fn load_messages(&self, session_id: &str) -> Result<Vec<MessageEntry>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare("SELECT role, content FROM messages WHERE session_id = ? ORDER BY id ASC")?;

        let records = stmt.query_map(params![session_id], |row| {
            Ok(MessageEntry {
                role: row.get(0)?,
                content: row.get(1)?,
            })
        })?;

        let mut out = Vec::new();
        for r in records {
            out.push(r?);
        }
        Ok(out)
    }

    fn clear_messages(&self, session_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "DELETE FROM messages WHERE session_id = ?",
            params![session_id],
        )?;
        Ok(())
    }

    fn clear_autosaved(&self, session_id: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sid) = session_id {
            conn.execute(
                "DELETE FROM memories WHERE key LIKE 'autosave_%' AND session_id = ?1",
                params![sid],
            )?;
        } else {
            conn.execute("DELETE FROM memories WHERE key LIKE 'autosave_%'", [])?;
        }
        Ok(())
    }
}
