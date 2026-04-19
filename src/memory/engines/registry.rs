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
    capabilities: BackendCapabilities { supports_keyword_rank: true, supports_session_store: true, supports_transactions: true, supports_outbox: true },
    needs_db_path: true,
    needs_workspace: false,
};

const MARKDOWN_BACKEND: BackendDescriptor = BackendDescriptor {
    name: "markdown",
    label: "Markdown files - simple, human-readable",
    auto_save_default: true,
    capabilities: BackendCapabilities { supports_keyword_rank: false, supports_session_store: false, supports_transactions: false, supports_outbox: false },
    needs_db_path: false,
    needs_workspace: true,
};

const LUCID_BACKEND: BackendDescriptor = BackendDescriptor {
    name: "lucid",
    label: "Lucid - SQLite + cross-project memory sync via lucid CLI",
    auto_save_default: true,
    capabilities: BackendCapabilities { supports_keyword_rank: true, supports_session_store: true, supports_transactions: true, supports_outbox: true },
    needs_db_path: true,
    needs_workspace: true,
};

const MEMORY_BACKEND: BackendDescriptor = BackendDescriptor {
    name: "memory",
    label: "In-memory LRU - no persistence, ideal for testing",
    auto_save_default: false,
    capabilities: BackendCapabilities { supports_keyword_rank: false, supports_session_store: false, supports_transactions: false, supports_outbox: false },
    needs_db_path: false,
    needs_workspace: false,
};

const REDIS_BACKEND: BackendDescriptor = BackendDescriptor {
    name: "redis",
    label: "Redis - distributed in-memory store with optional TTL",
    auto_save_default: true,
    capabilities: BackendCapabilities { supports_keyword_rank: false, supports_session_store: false, supports_transactions: false, supports_outbox: false },
    needs_db_path: false,
    needs_workspace: false,
};

const LANCEDB_BACKEND: BackendDescriptor = BackendDescriptor {
    name: "lancedb",
    label: "LanceDB - SQLite + vector-augmented recall",
    auto_save_default: true,
    capabilities: BackendCapabilities { supports_keyword_rank: false, supports_session_store: false, supports_transactions: false, supports_outbox: false },
    needs_db_path: true,
    needs_workspace: false,
};

const API_BACKEND: BackendDescriptor = BackendDescriptor {
    name: "api",
    label: "HTTP API - delegate to external REST service",
    auto_save_default: true,
    capabilities: BackendCapabilities { supports_keyword_rank: false, supports_session_store: true, supports_transactions: false, supports_outbox: false },
    needs_db_path: false,
    needs_workspace: false,
};

const NONE_BACKEND: BackendDescriptor = BackendDescriptor {
    name: "none",
    label: "None - disable persistent memory",
    auto_save_default: false,
    capabilities: BackendCapabilities { supports_keyword_rank: false, supports_session_store: false, supports_transactions: false, supports_outbox: false },
    needs_db_path: false,
    needs_workspace: false,
};

const POSTGRES_BACKEND: BackendDescriptor = BackendDescriptor {
    name: "postgres",
    label: "PostgreSQL - remote/shared memory store",
    auto_save_default: true,
    capabilities: BackendCapabilities { supports_keyword_rank: false, supports_session_store: true, supports_transactions: true, supports_outbox: true },
    needs_db_path: false,
    needs_workspace: false,
};

// TODO: Use conditional compilation (cfg) or features to enable/disable backends
pub const ALL_BACKENDS: &[BackendDescriptor] = &[
    MARKDOWN_BACKEND,
    API_BACKEND,
    MEMORY_BACKEND,
    NONE_BACKEND,
    SQLITE_BACKEND,
    LUCID_BACKEND,
    REDIS_BACKEND,
    LANCEDB_BACKEND,
    POSTGRES_BACKEND,
];

pub const KNOWN_BACKEND_NAMES: &[&str] = &[
    "none",
    "markdown",
    "memory",
    "api",
    "sqlite",
    "lucid",
    "redis",
    "lancedb",
    "postgres",
];

pub fn find_backend(name: &str) -> Option<&'static BackendDescriptor> {
    ALL_BACKENDS.iter().find(|desc| desc.name == name)
}

pub fn is_known_backend(name: &str) -> bool {
    KNOWN_BACKEND_NAMES.contains(&name)
}
