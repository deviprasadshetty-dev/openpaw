use serde::{Deserialize, Serialize};

// ── Autonomy Level ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AutonomyLevel {
    NoAutonomy,
    Steerable,
    #[default]
    Supervised,
    Autonomous,
}

// ── Named agent config (for agents map in JSON) ──────────────────────────────

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

// ── Session Config ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatType {
    Direct,
    Group,
    Channel,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PeerRef {
    pub kind: ChatType,
    pub id: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BindingMatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer: Option<PeerRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentBinding {
    pub agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default)]
    pub r#match: BindingMatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum DmScope {
    /// Single shared session for all DMs.
    Main,
    /// One session per peer across all channels.
    PerPeer,
    /// One session per (channel, peer) pair (default).
    #[default]
    PerChannelPeer,
    /// One session per (account, channel, peer) triple.
    PerAccountChannelPeer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchedBy {
    Peer,
    ParentPeer,
    GuildRoles,
    Guild,
    Team,
    Account,
    ChannelOnly,
    Default,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResolvedRoute {
    pub agent_id: String,
    pub channel: String,
    pub account_id: String,
    pub session_key: String,
    pub main_session_key: String,
    pub matched_by: MatchedBy,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RouteInput {
    pub channel: String,
    pub account_id: String,
    pub peer: Option<PeerRef>,
    pub parent_peer: Option<PeerRef>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
    #[serde(default)]
    pub member_role_ids: Vec<String>,
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
    /// Maximum inbound message size in bytes. Messages exceeding this are
    /// rejected before they reach the agent. Default: 50,000 bytes.
    /// Set to 0 to disable the limit.
    #[serde(default = "default_max_message_bytes")]
    pub max_message_bytes: usize,
    /// Maximum number of active sessions to keep in memory before the oldest are
    /// evicted to free up resources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_sessions: Option<usize>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            dm_scope: DmScope::default(),
            idle_minutes: default_idle_minutes(),
            identity_links: Vec::new(),
            typing_interval_secs: default_typing_interval_secs(),
            max_message_bytes: default_max_message_bytes(),
            max_sessions: Some(1000),
        }
    }
}

fn default_idle_minutes() -> u32 {
    60
}

fn default_typing_interval_secs() -> u32 {
    5
}

fn default_max_message_bytes() -> usize {
    50_000
}

// ── HTTP request config ─────────────────────────────────────────────────────

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
            search_fallback_providers: default_search_fallback_providers(),
            brave_search_api_key: None,
        }
    }
}

fn default_search_fallback_providers() -> Vec<String> {
    // gemini_cli is the primary provider; duckduckgo is kept as a last-resort fallback
    // only if gemini CLI is not installed. Brave requires an API key so is not a default.
    vec!["duckduckgo".to_string()]
}

fn default_http_max_response_size() -> u32 {
    1_000_000
}

fn default_http_timeout_secs() -> u64 {
    30
}

fn default_search_provider() -> String {
    "gemini_cli".to_string()
}

// ── OpenCode CLI config ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpencodeCliConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_opencode_binary")]
    pub binary: String,
    #[serde(default = "default_opencode_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_opencode_max_output_bytes")]
    pub max_output_bytes: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attach_url: Option<String>,
}

impl Default for OpencodeCliConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            binary: default_opencode_binary(),
            timeout_secs: default_opencode_timeout_secs(),
            max_output_bytes: default_opencode_max_output_bytes(),
            attach_url: None,
        }
    }
}

fn default_opencode_binary() -> String {
    "opencode".to_string()
}

fn default_opencode_timeout_secs() -> u64 {
    180
}

fn default_opencode_max_output_bytes() -> u32 {
    1_000_000
}

// ── Browser config ──────────────────────────────────────────────────────────

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
    #[serde(default = "default_cdp_host")]
    pub cdp_host: String,
    #[serde(default = "default_cdp_port")]
    pub cdp_port: u16,
    #[serde(default)]
    pub cdp_auto_launch: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_dir: Option<String>,
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
            cdp_host: default_cdp_host(),
            cdp_port: default_cdp_port(),
            cdp_auto_launch: true,
            profile_dir: None,
            computer_use: BrowserComputerUseConfig::default(),
            allowed_domains: Vec::new(),
        }
    }
}

fn default_browser_backend() -> String {
    "cdp".to_string()
}

fn default_true() -> bool {
    true
}

fn default_native_webdriver_url() -> String {
    "http://127.0.0.1:9515".to_string()
}

fn default_cdp_port() -> u16 {
    9222
}

fn default_cdp_host() -> String {
    "127.0.0.1".to_string()
}

// ── Composio config ─────────────────────────────────────────────────────────

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

// ── Hardware config ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum HardwareTransport {
    #[default]
    None,
    Native,
    Serial,
    Probe,
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

// ── Memory config ───────────────────────────────────────────────────────────

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

// ── Channels configs ─────────────────────────────────────────────────────────

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
pub struct WhatsAppNativeConfig {
    #[serde(default = "default_account_id")]
    pub account_id: String,
    pub bridge_url: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub auto_start: bool,
    pub bridge_dir: Option<String>,
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
    #[serde(default)]
    pub whatsapp_native: Vec<WhatsAppNativeConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook: Option<WebhookConfig>,
}

// ── MCP config ──────────────────────────────────────────────────────────────

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
    #[serde(default = "default_true")]
    pub always_inherit: bool,
    #[serde(default)]
    pub inherit: Vec<String>,
}

// ── Pushover config ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PushoverConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_key: Option<String>,
}

// ── Email config ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct EmailConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Override SMTP host (auto-detected from email domain if not set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smtp_host: Option<String>,
    /// Override SMTP port (default: 587).
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    /// Override IMAP host (auto-detected from email domain if not set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imap_host: Option<String>,
    /// Override IMAP port (default: 993).
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
}

fn default_smtp_port() -> u16 {
    587
}

fn default_imap_port() -> u16 {
    993
}

impl EmailConfig {
    /// Resolve SMTP host from explicit override or auto-detect from email domain.
    pub fn resolve_smtp_host(&self) -> String {
        if let Some(ref h) = self.smtp_host {
            return h.clone();
        }
        Self::detect_smtp(self.address.as_deref())
    }

    /// Resolve IMAP host from explicit override or auto-detect from email domain.
    pub fn resolve_imap_host(&self) -> String {
        if let Some(ref h) = self.imap_host {
            return h.clone();
        }
        Self::detect_imap(self.address.as_deref())
    }

    fn detect_smtp(address: Option<&str>) -> String {
        let domain = address
            .and_then(|a| a.split('@').last())
            .unwrap_or("gmail.com");
        match domain {
            "gmail.com" => "smtp.gmail.com".to_string(),
            "outlook.com" | "hotmail.com" | "live.com" => "smtp.office365.com".to_string(),
            "yahoo.com" | "yahoo.co.in" | "yahoo.co.uk" => "smtp.mail.yahoo.com".to_string(),
            "icloud.com" | "me.com" | "mac.com" => "smtp.mail.me.com".to_string(),
            _ => format!("smtp.{}", domain),
        }
    }

    fn detect_imap(address: Option<&str>) -> String {
        let domain = address
            .and_then(|a| a.split('@').last())
            .unwrap_or("gmail.com");
        match domain {
            "gmail.com" => "imap.gmail.com".to_string(),
            "outlook.com" | "hotmail.com" | "live.com" => "outlook.office365.com".to_string(),
            "yahoo.com" | "yahoo.co.in" | "yahoo.co.uk" => "imap.mail.yahoo.com".to_string(),
            "icloud.com" | "me.com" | "mac.com" => "imap.mail.me.com".to_string(),
            _ => format!("imap.{}", domain),
        }
    }
}

// ── Reliability config ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReliabilityConfig {
    #[serde(default = "default_scheduler_poll_secs")]
    pub scheduler_poll_secs: u64,
    #[serde(default = "default_heartbeat_interval_minutes")]
    pub heartbeat_interval_minutes: u64,
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

impl Default for ReliabilityConfig {
    fn default() -> Self {
        Self {
            // 1s polling for responsive short-delay reminders (e.g. 10s, 1m)
            scheduler_poll_secs: 1,
            heartbeat_interval_minutes: 30,
            timezone: "UTC".to_string(),
        }
    }
}

fn default_scheduler_poll_secs() -> u64 {
    1
}

fn default_heartbeat_interval_minutes() -> u64 {
    30
}

fn default_timezone() -> String {
    "UTC".to_string()
}

// ── Scheduler config ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SchedulerConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_tasks")]
    pub max_tasks: usize,
    #[serde(default = "default_agent_timeout_secs")]
    pub agent_timeout_secs: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_tasks: 1000,
            agent_timeout_secs: 300,
        }
    }
}

fn default_max_tasks() -> usize {
    1000
}

fn default_agent_timeout_secs() -> u64 {
    300
}

// ── Task-based model routing ─────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TaskModelsConfig {
    #[serde(default)]
    pub chat: Option<String>,
    #[serde(default)]
    pub tool_use: Option<String>,
    #[serde(default)]
    pub summarize: Option<String>,
    #[serde(default)]
    pub greeting: Option<String>,
    #[serde(default)]
    pub cron: Option<String>,
    #[serde(default)]
    pub subagent: Option<String>,
    #[serde(default)]
    pub heartbeat: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
}

impl Default for TaskModelsConfig {
    fn default() -> Self {
        Self {
            chat: None,
            tool_use: None,
            summarize: None,
            greeting: None,
            cron: None,
            subagent: None,
            heartbeat: None,
            event: None,
        }
    }
}

impl TaskModelsConfig {
    pub fn to_map(&self) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        if let Some(ref v) = self.chat {
            map.insert("chat".to_string(), v.clone());
        }
        if let Some(ref v) = self.tool_use {
            map.insert("tool_use".to_string(), v.clone());
        }
        if let Some(ref v) = self.summarize {
            map.insert("summarize".to_string(), v.clone());
        }
        if let Some(ref v) = self.greeting {
            map.insert("greeting".to_string(), v.clone());
        }
        if let Some(ref v) = self.cron {
            map.insert("cron".to_string(), v.clone());
        }
        if let Some(ref v) = self.subagent {
            map.insert("subagent".to_string(), v.clone());
        }
        if let Some(ref v) = self.heartbeat {
            map.insert("heartbeat".to_string(), v.clone());
        }
        if let Some(ref v) = self.event {
            map.insert("event".to_string(), v.clone());
        }
        map
    }
}

// ── SkillMint config ─────────────────────────────────────────────────────────

fn default_mint_min_duration() -> u64 {
    60
}
fn default_mint_high_importance_secs() -> u64 {
    300
}
fn default_mint_min_tasks() -> usize {
    3
}
fn default_skillmint_true() -> bool {
    true
}

/// Configuration for the SkillMint self-learning system.
///
/// SkillMint watches for successful multi-step plans and automatically
/// distils them into reusable skill files stored in `skills/minted/`.
/// A cheap secondary model handles distillation to keep costs low.
/// An optional GitHub-backed SkillVault syncs minted skills so they
/// survive agent wipes and can be restored on new instances instantly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMintConfig {
    /// Enable automatic skill minting after successful plans (default: true).
    #[serde(default = "default_skillmint_true")]
    pub enabled: bool,
    /// Minimum plan duration in seconds before a mint is considered. Default: 60 s.
    #[serde(default = "default_mint_min_duration")]
    pub min_duration_secs: u64,
    /// Minimum sub-task count before minting is considered. Default: 3.
    #[serde(default = "default_mint_min_tasks")]
    pub min_tasks: usize,
    /// Model to use for the distillation LLM call.
    /// If None (default), SkillMint reuses the user's already-configured
    /// secondary model: task_models.summarize → task_models.subagent → default model.
    /// No need to set this unless you want to override with a specific model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distill_model: Option<String>,
    /// Duration (secs) above which a mint is flagged High-importance (approval required).
    /// Default: 300 s (5 min).
    #[serde(default = "default_mint_high_importance_secs")]
    pub high_importance_secs: u64,
    /// GitHub repo for minted skills, e.g. "alice/openpaw-skills". None = local-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vault_repo: Option<String>,
    /// GitHub personal access token with `repo` scope for SkillVault sync.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vault_token: Option<String>,
    /// Auto-create the GitHub repo if it does not exist (default: true).
    #[serde(default = "default_skillmint_true")]
    pub vault_auto_create: bool,
    /// Make the auto-created repo private (default: true).
    #[serde(default = "default_skillmint_true")]
    pub vault_private: bool,
}

impl Default for SkillMintConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_duration_secs: default_mint_min_duration(),
            min_tasks: default_mint_min_tasks(),
            distill_model: None,
            high_importance_secs: default_mint_high_importance_secs(),
            vault_repo: None,
            vault_token: None,
            vault_auto_create: true,
            vault_private: true,
        }
    }
}
