//! Build-time configuration options
//! These are set at compile time via build.rs or environment variables

/// Version of the application
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Application name
pub const APP_NAME: &str = "openpaw";

/// Whether this is a debug build
pub const DEBUG: bool = cfg!(debug_assertions);

/// Target platform
pub const TARGET: &str = match option_env!("TARGET") {
    Some(t) => t,
    None => "unknown",
};
