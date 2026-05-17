pub struct BackendCapabilities {
    pub supports_keyword_rank: bool,
    pub supports_session_store: bool,
    pub supports_transactions: bool,
    pub supports_outbox: bool,
}

pub struct BackendDescriptor {
    pub name: &'static str,
    pub label: &'static str,
    pub auto_save_default: bool,
    pub capabilities: BackendCapabilities,
    pub needs_db_path: bool,
    pub needs_workspace: bool,
}

const SQLITE_BACKEND: BackendDescriptor = BackendDescriptor {
    name: "sqlite",
    label: "SQLite with FTS5 search (recommended)",
    auto_save_default: true,
    capabilities: BackendCapabilities {
        supports_keyword_rank: true,
        supports_session_store: true,
        supports_transactions: true,
        supports_outbox: true,
    },
    needs_db_path: true,
    needs_workspace: false,
};

const MARKDOWN_BACKEND: BackendDescriptor = BackendDescriptor {
    name: "markdown",
    label: "Markdown files - simple, human-readable",
    auto_save_default: true,
    capabilities: BackendCapabilities {
        supports_keyword_rank: false,
        supports_session_store: false,
        supports_transactions: false,
        supports_outbox: false,
    },
    needs_db_path: false,
    needs_workspace: true,
};

const NONE_BACKEND: BackendDescriptor = BackendDescriptor {
    name: "none",
    label: "None - disable persistent memory",
    auto_save_default: false,
    capabilities: BackendCapabilities {
        supports_keyword_rank: false,
        supports_session_store: false,
        supports_transactions: false,
        supports_outbox: false,
    },
    needs_db_path: false,
    needs_workspace: false,
};

pub const ALL_BACKENDS: &[BackendDescriptor] = &[MARKDOWN_BACKEND, NONE_BACKEND, SQLITE_BACKEND];

pub const KNOWN_BACKEND_NAMES: &[&str] = &["none", "markdown", "sqlite"];

pub fn find_backend(name: &str) -> Option<&'static BackendDescriptor> {
    ALL_BACKENDS.iter().find(|desc| desc.name == name)
}

pub fn is_known_backend(name: &str) -> bool {
    KNOWN_BACKEND_NAMES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_only_advertises_runtime_backends() {
        assert!(is_known_backend("none"));
        assert!(is_known_backend("markdown"));
        assert!(is_known_backend("sqlite"));
        assert!(!is_known_backend("redis"));
        assert!(!is_known_backend("lancedb"));
        assert!(!is_known_backend("api"));
        assert!(!is_known_backend("lucid"));
    }
}
