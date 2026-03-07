use std::env;
use std::path::PathBuf;

/// Returns the user's home directory.
/// Windows: USERPROFILE -> HOMEDRIVE+HOMEPATH
/// Unix: HOME
pub fn get_home_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        if let Ok(home) = env::var("USERPROFILE") {
            return Some(PathBuf::from(home));
        }
        let drive = env::var("HOMEDRIVE").ok()?;
        let path = env::var("HOMEPATH").ok()?;
        return Some(PathBuf::from(format!("{}{}", drive, path)));
    } else {
        env::var("HOME").ok().map(PathBuf::from)
    }
}

/// Returns the system temp directory.
pub fn get_temp_dir() -> PathBuf {
    env::temp_dir()
}

/// Returns the platform shell for executing commands.
pub fn get_shell() -> &'static str {
    if cfg!(target_os = "windows") {
        "cmd.exe"
    } else {
        "/bin/sh"
    }
}

/// Returns the shell flag for passing a command string.
pub fn get_shell_flag() -> &'static str {
    if cfg!(target_os = "windows") {
        "/c"
    } else {
        "-c"
    }
}

/// Cross-platform wrapper over std::env::var that returns Option instead of Result.
pub fn get_env_or_null(name: &str) -> Option<String> {
    env::var(name).ok()
}
