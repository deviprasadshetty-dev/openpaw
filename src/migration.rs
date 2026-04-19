use crate::config::Config;
use anyhow::{Result, anyhow};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergePolicy {
    SkipExisting,
    OverwriteNewer,
    RenameConflicts,
}

#[derive(Debug, Default)]
pub struct MigrationStats {
    pub from_sqlite: usize,
    pub from_markdown: usize,
    pub imported: usize,
    pub skipped_unchanged: usize,
    pub renamed_conflicts: usize,
    pub overwritten: usize,
    pub config_migrated: bool,
    pub backup_path: Option<String>,
}

pub struct SourceEntry {
    pub key: String,
    pub content: String,
    pub category: String,
}

pub struct Migration;

impl Migration {
    pub fn migrate_openclaw(
        _config: &Config,
        source_path: Option<&str>,
        _dry_run: bool,
    ) -> Result<MigrationStats> {
        Self::migrate_openclaw_with_policy(
            _config,
            source_path,
            _dry_run,
            MergePolicy::RenameConflicts,
        )
    }

    pub fn migrate_openclaw_with_policy(
        config: &Config,
        source_path: Option<&str>,
        dry_run: bool,
        _policy: MergePolicy,
    ) -> Result<MigrationStats> {
        let source = Self::resolve_openclaw_workspace(source_path)?;

        // Verify source exists
        if !source.exists() || !source.is_dir() {
            return Err(anyhow!("Source not found or not a directory: {:?}", source));
        }

        // Refuse self-migration
        let workspace_dir = Path::new(&config.workspace_dir).canonicalize()?;
        if source == workspace_dir {
            return Err(anyhow!("Cannot migrate from self (source == workspace)"));
        }

        let stats = MigrationStats::default();
        let mut _entries: Vec<SourceEntry> = Vec::new();
        let mut _seen_keys: HashSet<String> = HashSet::new();

        // Placeholder for reading logic
        // Self::read_openclaw_markdown_entries(&source, &mut entries, &mut stats)?;
        // Self::read_brain_db_entries(&source, &mut entries, &mut stats, &mut seen_keys)?;

        if dry_run {
            // stats.config_migrated = Self::migrate_openclaw_config(&source, &config.config_path, true)?;
            return Ok(stats);
        }

        // if !entries.is_empty() {
        // Create backup
        // Import entries
        // }

        // stats.config_migrated = Self::migrate_openclaw_config(&source, &config.config_path, false)?;

        Ok(stats)
    }

    fn resolve_openclaw_workspace(source_path: Option<&str>) -> Result<PathBuf> {
        if let Some(path) = source_path {
            return Ok(Path::new(path).canonicalize()?);
        }

        // Default locations check (e.g. ~/.openclaw)
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
            .map_err(|_| anyhow!("No home directory"))?;

        let default_path = home.join(".openclaw");
        if default_path.exists() {
            Ok(default_path)
        } else {
            Err(anyhow!(
                "No source path provided and default ~/.openclaw not found"
            ))
        }
    }
}
