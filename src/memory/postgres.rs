use super::{MemoryCategory, MemoryEntry, MemoryStore, MessageEntry, SessionStore};
use anyhow::Result;
use postgres::{Client, NoTls};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct PostgresMemory {
    client: Mutex<Client>,
    schema: String,
    table: String,
}

impl PostgresMemory {
    pub fn new(url: &str, schema: &str, table: &str) -> Result<Self> {
        Self::validate_identifier(schema)?;
        Self::validate_identifier(table)?;

        let mut client = Client::connect(url, NoTls)?;

        // Ensure schema exists
        client.execute(&format!("CREATE SCHEMA IF NOT EXISTS \"{}\"", schema), &[])?;

        // Ensure memories table exists
        // id, key, content, category, session_id, created_at, updated_at
        let table_fq = format!("\"{}\".\"{}\"", schema, table);
        let create_memories_sql = format!(
            r#"
            CREATE TABLE IF NOT EXISTS {} (
                id TEXT PRIMARY KEY,
                key TEXT NOT NULL UNIQUE,
                content TEXT NOT NULL,
                category TEXT NOT NULL DEFAULT 'core',
                session_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_{}_{}_category ON {} (category);
            CREATE INDEX IF NOT EXISTS idx_{}_{}_key ON {} (key);
            CREATE INDEX IF NOT EXISTS idx_{}_{}_session ON {} (session_id);
            "#,
            table_fq, schema, table, table_fq, schema, table, table_fq, schema, table, table_fq
        );
        client.batch_execute(&create_memories_sql)?;

        // Ensure messages table exists
        let messages_table_fq = format!("\"{}\".\"messages\"", schema);
        let create_messages_sql = format!(
            r#"
            CREATE TABLE IF NOT EXISTS {} (
                id SERIAL PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT DEFAULT (now()::text)
            );
            CREATE INDEX IF NOT EXISTS idx_{}_messages_session ON {} (session_id);
            "#,
            messages_table_fq, schema, messages_table_fq
        );
        client.batch_execute(&create_messages_sql)?;

        Ok(Self {
            client: Mutex::new(client),
            schema: schema.to_string(),
            table: table.to_string(),
        })
    }

    fn validate_identifier(name: &str) -> Result<()> {
        if name.is_empty() || name.len() > 63 {
            return Err(anyhow::anyhow!("Invalid identifier length"));
        }
        for c in name.chars() {
            if !c.is_alphanumeric() && c != '_' {
                return Err(anyhow::anyhow!("Invalid identifier characters"));
            }
        }
        Ok(())
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

    fn table_fq(&self) -> String {
        format!("\"{}\".\"{}\"", self.schema, self.table)
    }

    fn messages_table_fq(&self) -> String {
        format!("\"{}\".\"messages\"", self.schema)
    }
}

impl MemoryStore for PostgresMemory {
    fn name(&self) -> &str {
        "postgres"
    }

    fn store(
        &self,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
        _importance: Option<f64>,
    ) -> Result<()> {
        let mut client = self.client.lock().unwrap_or_else(|e| e.into_inner());
        let now = Self::now_str();
        let id = Self::nano_id();
        let cat_str = category.to_string();

        let sql = format!(
            "INSERT INTO {} (id, key, content, category, session_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (key) DO UPDATE SET
             content = EXCLUDED.content,
             category = EXCLUDED.category,
             session_id = EXCLUDED.session_id,
             updated_at = EXCLUDED.updated_at",
            self.table_fq()
        );

        client.execute(
            &sql,
            &[&id, &key, &content, &cat_str, &session_id, &now, &now],
        )?;
        Ok(())
    }

    fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        let mut client = self.client.lock().unwrap_or_else(|e| e.into_inner());
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        // Using ILIKE scoring similar to nullclaw implementation
        // score = (key ILIKE %q% ? 2.0 : 0.0) + (content ILIKE %q% ? 1.0 : 0.0)
        // We use parameters to prevent injection, but for the pattern we need to wrap input in %
        let pattern = format!("%{}%", trimmed.replace("%", "\\%").replace("_", "\\_"));

        let mut sql = format!(
            "SELECT id, key, content, category, created_at, session_id,
             (CASE WHEN key ILIKE $1 THEN 2.0 ELSE 0.0 END +
              CASE WHEN content ILIKE $1 THEN 1.0 ELSE 0.0 END) as score
             FROM {}
             WHERE (key ILIKE $1 OR content ILIKE $1)",
            self.table_fq()
        );

        let limit_i64 = limit as i64;
        let rows = if let Some(sid) = session_id {
            sql.push_str(" AND session_id = $2 ORDER BY score DESC LIMIT $3");
            client.query(&sql, &[&pattern, &sid, &limit_i64])?
        } else {
            sql.push_str(" ORDER BY score DESC LIMIT $2");
            client.query(&sql, &[&pattern, &limit_i64])?
        };

        let mut results = Vec::new();
        for row in rows {
            let cat_str: String = row.get(3);
            results.push(MemoryEntry {
                id: row.get(0),
                key: row.get(1),
                content: row.get(2),
                category: MemoryCategory::from_str(&cat_str),
                timestamp: row.get(4),
                session_id: row.get(5),
                score: Some(row.get(6)),
                importance: 0.5,
                embedding: None,
            });
        }

        Ok(results)
    }

    fn get(&self, key: &str) -> Result<Option<MemoryEntry>> {
        let mut client = self.client.lock().unwrap_or_else(|e| e.into_inner());
        let sql = format!(
            "SELECT id, key, content, category, created_at, session_id FROM {} WHERE key = $1",
            self.table_fq()
        );
        let row = client.query_opt(&sql, &[&key])?;

        if let Some(row) = row {
            let cat_str: String = row.get(3);
            Ok(Some(MemoryEntry {
                id: row.get(0),
                key: row.get(1),
                content: row.get(2),
                category: MemoryCategory::from_str(&cat_str),
                timestamp: row.get(4),
                session_id: row.get(5),
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
        let mut client = self.client.lock().unwrap_or_else(|e| e.into_inner());
        let mut sql = format!(
            "SELECT id, key, content, category, created_at, session_id FROM {}",
            self.table_fq()
        );

        let rows = match (category, session_id) {
            (Some(cat), Some(sid)) => {
                sql.push_str(" WHERE category = $1 AND session_id = $2 ORDER BY updated_at DESC");
                client.query(&sql, &[&cat.to_string(), &sid])?
            }
            (Some(cat), None) => {
                sql.push_str(" WHERE category = $1 ORDER BY updated_at DESC");
                client.query(&sql, &[&cat.to_string()])?
            }
            (None, Some(sid)) => {
                sql.push_str(" WHERE session_id = $1 ORDER BY updated_at DESC");
                client.query(&sql, &[&sid])?
            }
            (None, None) => {
                sql.push_str(" ORDER BY updated_at DESC");
                client.query(&sql, &[])?
            }
        };

        let mut results = Vec::new();
        for row in rows {
            let cat_str: String = row.get(3);
            results.push(MemoryEntry {
                id: row.get(0),
                key: row.get(1),
                content: row.get(2),
                category: MemoryCategory::from_str(&cat_str),
                timestamp: row.get(4),
                session_id: row.get(5),
                score: None,
                importance: 0.5,
                embedding: None,
            });
        }
        Ok(results)
    }

    fn forget(&self, key: &str) -> Result<bool> {
        let mut client = self.client.lock().unwrap_or_else(|e| e.into_inner());
        let sql = format!("DELETE FROM {} WHERE key = $1", self.table_fq());
        let count = client.execute(&sql, &[&key])?;
        Ok(count > 0)
    }

    fn count(&self) -> Result<usize> {
        let mut client = self.client.lock().unwrap_or_else(|e| e.into_inner());
        let sql = format!("SELECT COUNT(*) FROM {}", self.table_fq());
        let row = client.query_one(&sql, &[])?;
        let count: i64 = row.get(0);
        Ok(count as usize)
    }

    fn health_check(&self) -> bool {
        let mut client = self.client.lock().unwrap_or_else(|e| e.into_inner());
        match client.query_one("SELECT 1", &[]) {
            Ok(_) => true,
            Err(_) => false,
        }
    }
}

impl SessionStore for PostgresMemory {
    fn save_message(&self, session_id: &str, role: &str, content: &str) -> Result<()> {
        let mut client = self.client.lock().unwrap_or_else(|e| e.into_inner());
        let sql = format!(
            "INSERT INTO {} (session_id, role, content) VALUES ($1, $2, $3)",
            self.messages_table_fq()
        );
        client.execute(&sql, &[&session_id, &role, &content])?;
        Ok(())
    }

    fn load_messages(&self, session_id: &str) -> Result<Vec<MessageEntry>> {
        let mut client = self.client.lock().unwrap_or_else(|e| e.into_inner());
        let sql = format!(
            "SELECT role, content FROM {} WHERE session_id = $1 ORDER BY id ASC",
            self.messages_table_fq()
        );
        let rows = client.query(&sql, &[&session_id])?;
        let mut out = Vec::new();
        for r in rows {
            out.push(MessageEntry {
                role: r.get(0),
                content: r.get(1),
            });
        }
        Ok(out)
    }

    fn clear_messages(&self, session_id: &str) -> Result<()> {
        let mut client = self.client.lock().unwrap_or_else(|e| e.into_inner());
        let sql = format!(
            "DELETE FROM {} WHERE session_id = $1",
            self.messages_table_fq()
        );
        client.execute(&sql, &[&session_id])?;
        Ok(())
    }

    fn clear_autosaved(&self, session_id: Option<&str>) -> Result<()> {
        let mut client = self.client.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sid) = session_id {
            let sql = format!(
                "DELETE FROM {} WHERE key LIKE 'autosave_%' AND session_id = $1",
                self.table_fq()
            );
            client.execute(&sql, &[&sid])?;
        } else {
            let sql = format!(
                "DELETE FROM {} WHERE key LIKE 'autosave_%'",
                self.table_fq()
            );
            client.execute(&sql, &[])?;
        }
        Ok(())
    }
}
