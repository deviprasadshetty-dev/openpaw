pub trait RuntimeAdapter: Send + Sync {
    fn name(&self) -> &str;
    fn has_shell_access(&self) -> bool;
    fn has_filesystem_access(&self) -> bool;
    fn storage_path(&self) -> &str;
    fn supports_long_running(&self) -> bool;
    fn memory_budget(&self) -> u64;
}

pub struct NativeRuntime;

impl RuntimeAdapter for NativeRuntime {
    fn name(&self) -> &str {
        "native"
    }

    fn has_shell_access(&self) -> bool {
        true
    }

    fn has_filesystem_access(&self) -> bool {
        true
    }

    fn storage_path(&self) -> &str {
        ".nullclaw"
    }

    fn supports_long_running(&self) -> bool {
        true
    }

    fn memory_budget(&self) -> u64 {
        0 // Unlimited
    }
}
