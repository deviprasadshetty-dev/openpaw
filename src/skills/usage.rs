/// Skill usage tracking — persistent database for per-skill statistics.
///
/// Hermes-style skill usage tracking: every time a skill is used, viewed,
/// patched, created, or archived, the event is recorded in a SQLite database
/// so the curator and reporting tools can make data-driven decisions.
///
/// Tracks:
///   - use_count: number of times the skill was actively invoked
///   - view_count: number of times the skill was listed/read
///   - patch_count: number of times the skill was edited
///   - create_count: number of times the skill appeared (created/installed)
///   - activity_count: total activity events
///   - last_activity_at: ISO timestamp of most recent activity
///   - created_at: ISO timestamp of first appearance
///   - state: "active" | "stale" | "archived"
///   - pinned: whether the skill bypasses auto-transitions
///   - is_agent_created: whether the skill was created by the agent (vs bundled)
use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Skill lifecycle states (matches Hermes' skill_usage.STATE_*)
pub const STATE_ACTIVE: &str = "active";
pub const STATE_STALE: &str = "stale";
pub const STATE_ARCHIVED: &str = "archived";

/// Activity event types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Use,
    View,
    Patch,
    Create,
    Archive,
    Reactivate,
    MarkStale,
    Pin,
    Unpin,
}

impl ActivityKind {
    fn as_str(&self) -> &'static str {
        match self {
            ActivityKind::Use => "use",
            ActivityKind::View => "view",
            ActivityKind::Patch => "patch",
            ActivityKind::Create => "create",
            ActivityKind::Archive => "archive",
            ActivityKind::Reactivate => "reactivate",
            ActivityKind::MarkStale => "mark_stale",
            ActivityKind::Pin => "pin",
            ActivityKind::Unpin => "unpin",
        }
    }
}

/// A row from the skill_usage_report query.
#[derive(Debug, Clone)]
pub struct SkillUsageRow {
    pub name: String,
    pub state: String,
    pub pinned: bool,
    pub is_agent_created: bool,
    pub use_count: u32,
    pub view_count: u32,
    pub patch_count: u32,
    pub create_count: u32,
    pub activity_count: u32,
    pub last_activity_at: Option<String>,
    pub created_at: Option<String>,
}

/// Persistent skill usage database backed by SQLite.
pub struct SkillUsageDB {
    conn: Mutex<Connection>,
}

impl SkillUsageDB {
    /// Open (or create) the usage database at `db_path`.
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;

        // Enable WAL mode for concurrent reads
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS skill_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                skill_name TEXT NOT NULL,
                kind TEXT NOT NULL,
                timestamp_secs INTEGER NOT NULL,
                details TEXT DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_skill_events_name ON skill_events(skill_name);
            CREATE INDEX IF NOT EXISTS idx_skill_events_kind ON skill_events(kind);
            CREATE INDEX IF NOT EXISTS idx_skill_events_ts ON skill_events(timestamp_secs);

            CREATE TABLE IF NOT EXISTS skill_meta (
                skill_name TEXT PRIMARY KEY,
                state TEXT NOT NULL DEFAULT 'active',
                pinned INTEGER NOT NULL DEFAULT 0,
                is_agent_created INTEGER NOT NULL DEFAULT 0,
                created_at_secs INTEGER,
                last_activity_secs INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_skill_meta_state ON skill_meta(state);"
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Record a skill activity event.
    pub fn record(&self, skill_name: &str, kind: ActivityKind, details: &str) -> Result<()> {
        let now = now_secs();
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO skill_events (skill_name, kind, timestamp_secs, details) VALUES (?1, ?2, ?3, ?4)",
            params![skill_name, kind.as_str(), now, details],
        )?;

        // Upsert skill_meta
        let is_create = matches!(kind, ActivityKind::Create);
        conn.execute(
            "INSERT INTO skill_meta (skill_name, state, pinned, is_agent_created, created_at_secs, last_activity_secs)
             VALUES (?1, 'active', 0, ?2, ?3, ?3)
             ON CONFLICT(skill_name) DO UPDATE SET
                last_activity_secs = ?3,
                created_at_secs = CASE WHEN ?4 THEN ?3 ELSE created_at_secs END",
            params![skill_name, is_create, now, is_create],
        )?;

        Ok(())
    }

    /// Record a skill use (invocation).
    pub fn record_use(&self, skill_name: &str) -> Result<()> {
        self.record(skill_name, ActivityKind::Use, "")
    }

    /// Record a skill view (listed or read).
    pub fn record_view(&self, skill_name: &str) -> Result<()> {
        self.record(skill_name, ActivityKind::View, "")
    }

    /// Record a skill patch (edit).
    pub fn record_patch(&self, skill_name: &str, details: &str) -> Result<()> {
        self.record(skill_name, ActivityKind::Patch, details)
    }

    /// Register a skill as agent-created (call on first discovery or creation).
    pub fn register_skill(&self, skill_name: &str, is_agent_created: bool) -> Result<()> {
        let now = now_secs();
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO skill_meta (skill_name, state, pinned, is_agent_created, created_at_secs, last_activity_secs)
             VALUES (?1, 'active', 0, ?2, ?3, ?3)
             ON CONFLICT(skill_name) DO UPDATE SET
                is_agent_created = MAX(is_agent_created, ?2)",
            params![skill_name, is_agent_created as i32, now],
        )?;
        Ok(())
    }

    /// Set skill state.
    pub fn set_state(&self, skill_name: &str, state: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE skill_meta SET state = ?2 WHERE skill_name = ?1",
            params![skill_name, state],
        )?;
        Ok(())
    }

    /// Set whether a skill is pinned.
    pub fn set_pinned(&self, skill_name: &str, pinned: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE skill_meta SET pinned = ?2 WHERE skill_name = ?1",
            params![skill_name, pinned as i32],
        )?;
        Ok(())
    }

    /// Check if a skill is pinned.
    pub fn is_pinned(&self, skill_name: &str) -> bool {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT pinned FROM skill_meta WHERE skill_name = ?1",
            params![skill_name],
            |row| row.get::<_, i32>(0),
        )
        .map(|v| v != 0)
        .unwrap_or(false)
    }

    /// Get the current state of a skill.
    pub fn get_state(&self, skill_name: &str) -> String {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT state FROM skill_meta WHERE skill_name = ?1",
            params![skill_name],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_else(|_| STATE_ACTIVE.to_string())
    }

    /// Report on all tracked skills with aggregated stats.
    pub fn full_report(&self) -> Result<Vec<SkillUsageRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT
                m.skill_name,
                m.state,
                m.pinned,
                m.is_agent_created,
                COALESCE(u.cnt, 0) AS use_count,
                COALESCE(v.cnt, 0) AS view_count,
                COALESCE(p.cnt, 0) AS patch_count,
                COALESCE(c.cnt, 0) AS create_count,
                COALESCE(u.cnt, 0) + COALESCE(v.cnt, 0) + COALESCE(p.cnt, 0) + COALESCE(c.cnt, 0) AS activity_count,
                CASE WHEN m.last_activity_secs IS NOT NULL THEN
                    datetime(m.last_activity_secs, 'unixepoch') ELSE NULL
                END AS last_activity_at,
                CASE WHEN m.created_at_secs IS NOT NULL THEN
                    datetime(m.created_at_secs, 'unixepoch') ELSE NULL
                END AS created_at
             FROM skill_meta m
             LEFT JOIN (SELECT skill_name, COUNT(*) AS cnt FROM skill_events WHERE kind='use' GROUP BY skill_name) u
                ON m.skill_name = u.skill_name
             LEFT JOIN (SELECT skill_name, COUNT(*) AS cnt FROM skill_events WHERE kind='view' GROUP BY skill_name) v
                ON m.skill_name = v.skill_name
             LEFT JOIN (SELECT skill_name, COUNT(*) AS cnt FROM skill_events WHERE kind='patch' GROUP BY skill_name) p
                ON m.skill_name = p.skill_name
             LEFT JOIN (SELECT skill_name, COUNT(*) AS cnt FROM skill_events WHERE kind='create' GROUP BY skill_name) c
                ON m.skill_name = c.skill_name
             ORDER BY activity_count DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(SkillUsageRow {
                name: row.get(0)?,
                state: row.get(1)?,
                pinned: row.get::<_, i32>(2)? != 0,
                is_agent_created: row.get::<_, i32>(3)? != 0,
                use_count: row.get::<_, i32>(4)? as u32,
                view_count: row.get::<_, i32>(5)? as u32,
                patch_count: row.get::<_, i32>(6)? as u32,
                create_count: row.get::<_, i32>(7)? as u32,
                activity_count: row.get::<_, i32>(8)? as u32,
                last_activity_at: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Report only on agent-created skills (for the curator).
    pub fn agent_created_report(&self) -> Result<Vec<SkillUsageRow>> {
        let all = self.full_report()?;
        Ok(all.into_iter().filter(|r| r.is_agent_created).collect())
    }

    /// Get stats for a specific skill.
    pub fn get_skill_stats(&self, skill_name: &str) -> Result<Option<SkillUsageRow>> {
        let all = self.full_report()?;
        Ok(all.into_iter().find(|r| r.name == skill_name))
    }

    /// Check if a skill exists in the meta table.
    pub fn has_skill(&self, skill_name: &str) -> bool {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT COUNT(*) FROM skill_meta WHERE skill_name = ?1",
            params![skill_name],
            |row| row.get::<_, i32>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false)
    }

    /// Scan all workspace skills and register any new ones in the database.
    /// Call this at startup or when skills change.
    pub fn sync_skill_names(&self, workspace_skill_names: &[String], is_agent_created_fn: impl Fn(&str) -> bool) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        for name in workspace_skill_names {
            let is_agent = is_agent_created_fn(name);
            // Register if not present
            conn.execute(
                "INSERT OR IGNORE INTO skill_meta (skill_name, state, pinned, is_agent_created, created_at_secs, last_activity_secs)
                 VALUES (?1, 'active', 0, ?2, ?3, ?3)",
                params![name, is_agent as i32, now_secs()],
            )?;
        }
        Ok(())
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Helper: format a skill usage row as a one-line summary for agent consumption.
pub fn format_skill_row(row: &SkillUsageRow) -> String {
    format!(
        "{name}  state={state}  pinned={pinned}  use={use_c}  view={view_c}  \
         patches={patch_c}  activity={act}  last_activity={last}",
        name = row.name,
        state = row.state,
        pinned = if row.pinned { "yes" } else { "no" },
        use_c = row.use_count,
        view_c = row.view_count,
        patch_c = row.patch_count,
        act = row.activity_count,
        last = row.last_activity_at.as_deref().unwrap_or("never"),
    )
}
