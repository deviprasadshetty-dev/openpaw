//! Porting utilities for cross-platform compatibility
//! This module provides platform-specific abstractions

use std::path::PathBuf;

/// Platform-specific path handling
pub fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// Check if running on Windows
pub fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

/// Check if running on macOS
pub fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

/// Check if running on Linux
pub fn is_linux() -> bool {
    cfg!(target_os = "linux")
}

/// Get platform name
pub fn platform_name() -> &'static str {
    if is_windows() {
        "windows"
    } else if is_macos() {
        "macos"
    } else if is_linux() {
        "linux"
    } else {
        "unknown"
    }
}

/// Platform-specific home directory
pub fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from).or_else(|| {
        if is_windows() {
            std::env::var("USERPROFILE").ok().map(PathBuf::from)
        } else {
            None
        }
    })
}

/// Platform-specific config directory
pub fn config_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".config"))
}

/// Platform-specific data directory
pub fn data_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".local").join("share"))
}
