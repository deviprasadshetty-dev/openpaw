use crate::config::Config;
use crate::daemon::DaemonState;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{error, info};

const HYGIENE_CHECK_INTERVAL_SECS: u64 = 60 * 60; // 1 hour
const DEFAULT_RETENTION_DAYS: u64 = 30;

pub fn hygiene_thread(state: Arc<std::sync::Mutex<DaemonState>>, config: Arc<Config>) {
    info!("Memory hygiene thread started");
    loop {
        if crate::daemon::is_shutdown_requested() {
            break;
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let should_run = {
            let guard = state.lock().unwrap();
            let time_since_hygiene = now.saturating_sub(guard.last_hygiene_at as u64);
            // Run every 12 hours like NullClaw
            time_since_hygiene >= (12 * 60 * 60)
        };

        if should_run {
            info!("Running memory hygiene pass...");
            if let Err(e) = run_hygiene_pass(&config) {
                error!("Memory hygiene pass failed: {}", e);
            } else {
                let mut guard = state.lock().unwrap();
                guard.last_hygiene_at = now as i64;
                info!("Memory hygiene pass completed.");
            }
        }

        std::thread::sleep(Duration::from_secs(HYGIENE_CHECK_INTERVAL_SECS));
    }
}

fn run_hygiene_pass(config: &Config) -> anyhow::Result<()> {
    // 1. Prune old SQLite conversation history if using sqlite backend
    if config.memory.backend == "sqlite" {
        let db_path = format!("{}/memory.db", config.workspace_dir);
        if Path::new(&db_path).exists() {
            prune_sqlite_history(&db_path, DEFAULT_RETENTION_DAYS)?;
        }
    }

    // 2. Archive old memories could be added here in the future

    Ok(())
}

fn prune_sqlite_history(db_path: &str, retention_days: u64) -> anyhow::Result<()> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path)?;

    let cutoff_ts =
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() - (retention_days * 24 * 60 * 60);

    // Assuming a schema similar to NullClaw's for session messages
    // OpenPaw's current sqlite memory might not have a formal 'messages' table yet
    // if it primarily uses it for KV-style memory. store.

    // Check if table exists first
    let table_exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='session_messages')",
        [],
        |row| row.get(0),
    ).unwrap_or(false);

    if table_exists {
        let deleted = conn.execute(
            "DELETE FROM session_messages WHERE created_at < ?",
            [cutoff_ts as i64],
        )?;
        if deleted > 0 {
            info!("Pruned {} old conversation messages from SQLite", deleted);
        }
    }

    Ok(())
}
