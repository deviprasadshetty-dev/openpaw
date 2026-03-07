use serde::{Deserialize, Serialize};

// ── Autonomy Level ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyLevel {
    NoAutonomy,
    Steerable,
    Supervised,
    Autonomous,
}

impl Default for AutonomyLevel {
    fn default() -> Self {
        Self::Supervised
    }
}

// ── Named agent config (for agents map in JSON) ────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NamedAgentConfig {
    pub name: String,
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
}

fn default_max_depth() -> u32 {
    3
}

// ── Session Config ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DmScope {
    /// Single shared session for all DMs.
    Main,
    /// One session per peer across all channels.
    PerPeer,
    /// One session per (channel, peer) pair (default).
    PerChannelPeer,
    /// One session per (account, channel, peer) triple.
    PerAccountChannelPeer,
}

impl Default for DmScope {
    fn default() -> Self {
        Self::PerChannelPeer
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IdentityLink {
    pub canonical: String,
    #[serde(default)]
    pub peers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionConfig {
    #[serde(default)]
    pub dm_scope: DmScope,
    #[serde(default = "default_idle_minutes")]
    pub idle_minutes: u32,
    #[serde(default)]
    pub identity_links: Vec<IdentityLink>,
    #[serde(default = "default_typing_interval_secs")]
    pub typing_interval_secs: u32,
}

fn default_idle_minutes() -> u32 {
    60
}

fn default_typing_interval_secs() -> u32 {
    5
}

// ── HTTP request config ─────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpRequestConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_http_max_response_size")]
    pub max_response_size: u32,
    #[serde(default = "default_http_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_base_url: Option<String>,
    #[serde(default = "default_search_provider")]
    pub search_provider: String,
    #[serde(default)]
    pub search_fallback_providers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brave_search_api_key: Option<String>,
}

impl Default for HttpRequestConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_response_size: default_http_max_response_size(),
            timeout_secs: default_http_timeout_secs(),
            allowed_domains: Vec::new(),
            search_base_url: None,
            search_provider: default_search_provider(),
            search_fallback_providers: Vec::new(),
            brave_search_api_key: None,
        }
    }
}

fn default_http_max_response_size() -> u32 {
    1_000_000
}

fn default_http_timeout_secs() -> u64 {
    30
}

fn default_search_provider() -> String {
    "auto".to_string()
}

// ── Browser config ──────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BrowserComputerUseConfig {
    #[serde(default = "default_browser_endpoint")]
    pub endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default = "default_browser_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub allow_remote_endpoint: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_coordinate_x: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_coordinate_y: Option<i64>,
}

impl Default for BrowserComputerUseConfig {
    fn default() -> Self {
        Self {
            endpoint: default_browser_endpoint(),
            api_key: None,
            timeout_ms: default_browser_timeout_ms(),
            allow_remote_endpoint: false,
            max_coordinate_x: None,
            max_coordinate_y: None,
        }
    }
}

fn default_browser_endpoint() -> String {
    "http://127.0.0.1:8787/v1/actions".to_string()
}

fn default_browser_timeout_ms() -> u64 {
    15_000
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BrowserConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    #[serde(default = "default_browser_backend")]
    pub backend: String,
    #[serde(default = "default_true")]
    pub native_headless: bool,
    #[serde(default = "default_native_webdriver_url")]
    pub native_webdriver_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_chrome_path: Option<String>,
    #[serde(default)]
    pub computer_use: BrowserComputerUseConfig,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            session_name: None,
            backend: default_browser_backend(),
            native_headless: true,
            native_webdriver_url: default_native_webdriver_url(),
            native_chrome_path: None,
            computer_use: BrowserComputerUseConfig::default(),
            allowed_domains: Vec::new(),
        }
    }
}

fn default_browser_backend() -> String {
    "agent_browser".to_string()
}

fn default_true() -> bool {
    true
}

fn default_native_webdriver_url() -> String {
    "http://127.0.0.1:9515".to_string()
}

// ── Composio config ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ComposioConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default = "default_composio_entity_id")]
    pub entity_id: String,
}

impl Default for ComposioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: None,
            entity_id: default_composio_entity_id(),
        }
    }
}

fn default_composio_entity_id() -> String {
    "default".to_string()
}

// ── Hardware config ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareTransport {
    None,
    Native,
    Serial,
    Probe,
}

impl Default for HardwareTransport {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HardwareConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub transport: HardwareTransport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_port: Option<String>,
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_target: Option<String>,
    #[serde(default)]
    pub workspace_datasheets: bool,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            transport: HardwareTransport::None,
            serial_port: None,
            baud_rate: default_baud_rate(),
            probe_target: None,
            workspace_datasheets: false,
        }
    }
}

fn default_baud_rate() -> u32 {
    115200
}

// ── Memory config ───────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryConfig {
    #[serde(default = "default_memory_backend")]
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            backend: default_memory_backend(),
            embedding_model: None,
        }
    }
}

fn default_memory_backend() -> String {
    "markdown".to_string()
}

// ── Channels configs ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramConfig {
    #[serde(default = "default_account_id")]
    pub account_id: String,
    /// Bot token from @BotFather (required)
    pub bot_token: String,
    /// Allowlist of usernames/user_ids that can interact with the bot
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// Allowlist for group chats (if different from private)
    #[serde(default)]
    pub group_allow_from: Vec<String>,
    /// Group policy: "allowlist" (default), "open", "disabled"
    #[serde(default = "default_group_policy")]
    pub group_policy: String,
    /// Reply in private even if message came from group
    #[serde(default = "default_true")]
    pub reply_in_private: bool,
    /// Optional SOCKS5/HTTP proxy URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WebhookConfig {}

fn default_account_id() -> String {
    "default".to_string()
}

fn default_group_policy() -> String {
    "allowlist".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ChannelsConfig {
    #[serde(default = "default_true")]
    pub cli: bool,
    #[serde(default)]
    pub telegram: Vec<TelegramConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook: Option<WebhookConfig>,
}

// ── MCP config ──────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerEnv {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<McpServerEnv>,
}
