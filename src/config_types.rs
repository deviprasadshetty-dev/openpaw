use serde::{Deserialize, Serialize};

// ── Autonomy Level ──────────────────────────────────────────────

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

// ── Named agent config (for agents map in JSON) ────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NamedAgentConfig {
    pub name: String,
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cheap_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cheap_model: Option<String>,
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
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            dm_scope: DmScope::default(),
            idle_minutes: default_idle_minutes(),
            identity_links: Vec::new(),
            typing_interval_secs: default_typing_interval_secs(),
        }
    }
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

// ── OpenCode CLI config ─────────────────────────────────────────

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

// ── Memory config ───────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryConfig {
    #[serde(default = "default_memory_backend")]
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            backend: default_memory_backend(),
            embedding_provider: None,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EmailConfig {
    #[serde(default = "default_account_id")]
    pub account_id: String,
    pub smtp_user: String,
    pub smtp_pass: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub imap_host: String,
    pub imap_port: u16,
}

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
    pub email: Vec<EmailConfig>,
    #[serde(default)]
    pub whatsapp_native: Vec<WhatsAppNativeConfig>,
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
    #[serde(default = "default_true")]
    pub always_inherit: bool,
    #[serde(default)]
    pub inherit: Vec<String>,
}
// ── Pushover config ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PushoverConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_key: Option<String>,
}
// ── Reliability config ──────────────────────────────────────────

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

// ── Scheduler config ────────────────────────────────────────────

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

// ── Skills config ───────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillsConfig {
    #[serde(default = "default_creation_nudge_interval")]
    pub creation_nudge_interval: u32,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            creation_nudge_interval: default_creation_nudge_interval(),
        }
    }
}

fn default_creation_nudge_interval() -> u32 {
    15
}

// ── Self-Learning config ────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SelfLearningConfig {
    #[serde(default = "default_memory_nudge_interval")]
    pub memory_nudge_interval: u32,
    #[serde(default = "default_memory_char_limit")]
    pub memory_char_limit: usize,
    #[serde(default = "default_user_char_limit")]
    pub user_char_limit: usize,
    #[serde(default = "default_flush_min_turns")]
    pub flush_min_turns: u32,
    #[serde(default = "default_dialectic_enabled")]
    pub dialectic_enabled: bool,
}

impl Default for SelfLearningConfig {
    fn default() -> Self {
        Self {
            memory_nudge_interval: default_memory_nudge_interval(),
            memory_char_limit: default_memory_char_limit(),
            user_char_limit: default_user_char_limit(),
            flush_min_turns: default_flush_min_turns(),
            dialectic_enabled: default_dialectic_enabled(),
        }
    }
}

fn default_memory_nudge_interval() -> u32 {
    10
}

fn default_memory_char_limit() -> usize {
    2200
}

fn default_user_char_limit() -> usize {
    1375
}

fn default_flush_min_turns() -> u32 {
    6
}

fn default_dialectic_enabled() -> bool {
    true
}

// ── Efficiency config (Hermes-style cost/token optimization) ────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EfficiencyConfig {
    /// Enable proactive context compression before hitting token limits.
    #[serde(default = "default_true")]
    pub proactive_compression: bool,
    /// Enable prompt caching to preserve prefix cache across turns.
    #[serde(default = "default_true")]
    pub prompt_caching: bool,
    /// Enable enhanced token estimation for accurate budget tracking.
    #[serde(default = "default_true")]
    pub accurate_token_estimation: bool,
    /// Per-turn token budget (0 = unlimited).
    #[serde(default = "default_turn_token_budget")]
    pub turn_token_budget: u64,
    /// Enable skill self-improvement based on execution feedback.
    #[serde(default = "default_true")]
    pub skill_self_improvement: bool,
    /// Minimum executions before a skill is considered for improvement.
    #[serde(default = "default_skill_min_executions")]
    pub skill_min_executions: usize,
    /// Enable zero-context-cost subagent delegation (minimal context).
    #[serde(default = "default_true")]
    pub minimal_subagent_context: bool,
    /// Enable periodic user modeling analysis (every N turns, 0 = off).
    #[serde(default = "default_user_modeling_interval")]
    pub user_modeling_interval: u32,
    /// Enable cross-session episodic memory search.
    #[serde(default = "default_true")]
    pub episodic_memory_search: bool,
}

impl Default for EfficiencyConfig {
    fn default() -> Self {
        Self {
            proactive_compression: true,
            prompt_caching: true,
            accurate_token_estimation: true,
            turn_token_budget: default_turn_token_budget(),
            skill_self_improvement: true,
            skill_min_executions: default_skill_min_executions(),
            minimal_subagent_context: true,
            user_modeling_interval: default_user_modeling_interval(),
            episodic_memory_search: true,
        }
    }
}

fn default_turn_token_budget() -> u64 {
    0 // Unlimited by default
}

fn default_skill_min_executions() -> usize {
    3
}

fn default_user_modeling_interval() -> u32 {
    5
}
