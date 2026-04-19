use std::path::{Component, Path};

#[cfg(unix)]
const SYSTEM_BLOCKED_PREFIXES: &[&str] = &[
    "/System",
    "/Library",
    "/bin",
    "/sbin",
    "/usr/bin",
    "/usr/sbin",
    "/usr/lib",
    "/usr/libexec",
    "/etc",
    "/private/etc",
    "/private/var",
    "/dev",
    "/boot",
    "/proc",
    "/sys",
];

#[cfg(windows)]
const SYSTEM_BLOCKED_PREFIXES: &[&str] = &[
    "C:\\Windows",
    "C:\\Program Files",
    "C:\\Program Files (x86)",
    "C:\\ProgramData",
    "C:\\System32",
    "C:\\Recovery",
];

pub fn is_path_safe(path: &str) -> bool {
    if path.contains('\0') {
        return false;
    }
    let p = Path::new(path);

    // Check for absolute paths if the tool expects relative workspace paths
    // But sometimes absolute paths are okay if they are within allowed directories
    // Here we just check for basic syntax safety

    for component in p.components() {
        if component == Component::ParentDir {
            return false; // ".." is strictly forbidden in input
        }
    }

    true
}

pub fn is_resolved_path_allowed(
    target_path: &Path,
    workspace_root: &Path,
    additional_allowed_paths: &[String],
) -> bool {
    // 1. Resolve the target path to its canonical absolute form (resolves symlinks)
    // If the path doesn't exist yet (e.g. writing a new file), we canonicalize the parent.
    let canonical_target = if target_path.exists() {
        match std::fs::canonicalize(target_path) {
            Ok(p) => p,
            Err(_) => return false, // Access denied or other error
        }
    } else {
        // For new files, check the parent directory
        if let Some(parent) = target_path.parent() {
            match std::fs::canonicalize(parent) {
                Ok(mut p) => {
                    if let Some(file_name) = target_path.file_name() {
                        p.push(file_name);
                        p
                    } else {
                        return false;
                    }
                }
                Err(_) => return false,
            }
        } else {
            return false;
        }
    };

    let canonical_workspace = match std::fs::canonicalize(workspace_root) {
        Ok(p) => p,
        Err(_) => return false, // Workspace must exist
    };

    // 2. Check System Blocklist on the canonical path
    let target_str = canonical_target.to_string_lossy();
    for prefix in SYSTEM_BLOCKED_PREFIXES {
        if target_str.starts_with(prefix) {
            return false;
        }
    }

    // 3. Check if inside Workspace
    if canonical_target.starts_with(&canonical_workspace) {
        return true;
    }

    // 4. Check Allowed External Paths
    for allowed in additional_allowed_paths {
        if let Ok(allowed_canon) = std::fs::canonicalize(allowed)
            && canonical_target.starts_with(&allowed_canon)
        {
            return true;
        }
    }

    false
}
