use crate::bus::{self, Bus};
use crate::channel_adapters::find_polling_descriptor;
use crate::channel_loop::{self, ChannelRuntime, PollingState};
use crate::channels::dispatch::{ChannelRegistry, run_outbound_dispatcher};
use crate::channels::telegram::TelegramChannel;
use crate::config::Config;
use crate::cron::CronScheduler;
use crate::gateway;
use crate::heartbeat::HeartbeatEngine;
use crate::providers::Provider;
use crate::providers::factory;
use crate::session::SessionManager;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use tracing::{error, info, warn};

const STATUS_FLUSH_SECONDS: u64 = 5;
const MAX_COMPONENTS: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ComponentStatus {
    pub name: String,
    pub running: bool,
    pub restart_count: u64,
    pub last_error: Option<String>,
    pub backoff_secs: u64,
    pub last_restart_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonState {
    pub started: bool,
    pub gateway_host: String,
    pub gateway_port: u16,
    pub components: Vec<ComponentStatus>,
}

impl Default for DaemonState {
    fn default() -> Self {
        Self {
            started: false,
            gateway_host: "127.0.0.1".to_string(),
            gateway_port: 3000,
            components: Vec::new(),
        }
    }
}

impl DaemonState {
    pub fn add_component(&mut self, name: &str) {
        if self.components.len() < MAX_COMPONENTS {
            self.components.push(ComponentStatus {
                name: name.to_string(),
                running: true,
                restart_count: 0,
                last_error: None,
                backoff_secs: 1,
                last_restart_at: None,
            });
        }
    }

    pub fn mark_error(&mut self, name: &str, err_msg: &str) {
        if let Some(comp) = self.components.iter_mut().find(|c| c.name == name) {
            comp.running = false;
            comp.last_error = Some(err_msg.to_string());
            comp.restart_count += 1;
            comp.backoff_secs = compute_backoff(comp.backoff_secs, 60);
        }
    }

    pub fn mark_running(&mut self, name: &str) {
        if let Some(comp) = self.components.iter_mut().find(|c| c.name == name) {
            comp.running = true;
            comp.last_error = None;
            comp.backoff_secs = 1; // Reset backoff on success
            comp.last_restart_at = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            );
        }
    }
}

pub fn state_file_path(config_path: &str) -> PathBuf {
    let path = PathBuf::from(config_path);
    if let Some(parent) = path.parent() {
        parent.join("daemon_state.json")
    } else {
        PathBuf::from("daemon_state.json")
    }
}

pub fn write_state_file(path: &PathBuf, state: &DaemonState) -> Result<()> {
    let json = serde_json::to_string_pretty(state)?;
    fs::write(path, json)?;
    Ok(())
}

pub fn compute_backoff(current_backoff: u64, max_backoff: u64) -> u64 {
    let doubled = current_backoff.saturating_mul(2);
    std::cmp::min(doubled, max_backoff)
}

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn request_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::Release);
}

pub fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Acquire)
}

pub async fn gateway_thread(config: Arc<Config>, state: Arc<std::sync::Mutex<DaemonState>>) {
    let name = "gateway";
    loop {
        if is_shutdown_requested() {
            break;
        }

        {
            let mut guard = state.lock().unwrap();
            guard.mark_running(name);
        }

        info!("Starting gateway...");
        let cfg = (*config).clone();
        if let Err(e) = gateway::serve(cfg).await {
            error!("Gateway failed: {}", e);
            let backoff;
            {
                let mut guard = state.lock().unwrap();
                guard.mark_error(name, &e.to_string());
                backoff = guard
                    .components
                    .iter()
                    .find(|c| c.name == name)
                    .map(|c| c.backoff_secs)
                    .unwrap_or(1);
            }
            tokio::time::sleep(Duration::from_secs(backoff)).await;
        } else {
            if is_shutdown_requested() {
                break;
            }
            warn!("Gateway exited normally, restarting in 1s...");
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

fn print_startup_info(config: &Config) {
    println!();
    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║           🐾 OpenPaw Agent Started Successfully!          ║");
    println!("╚═══════════════════════════════════════════════════════════╝");
    println!();

    // Web UI info
    let host = if config.gateway.allow_public_bind.unwrap_or(false) {
        "0.0.0.0".to_string()
    } else {
        config.gateway.host.clone()
    };
    println!("🌐 Web UI:    http://{}:{}", host, config.gateway.port);
    println!("             Configure settings and chat with your agent!");
    println!();

    // Telegram info
    if !config.channels.telegram.is_empty() {
        println!(
            "📱 Telegram: {} bot(s) configured",
            config.channels.telegram.len()
        );
        for (i, tg) in config.channels.telegram.iter().enumerate() {
            println!("   [{}] Account: {}", i + 1, tg.account_id);
            if !tg.allow_from.is_empty() {
                println!("       Allow from: {}", tg.allow_from.join(", "));
            }
        }
        println!("             Users can message your bot directly on Telegram!");
        println!();
    }

    // Other channels
    if !config.channels.whatsapp_native.is_empty() {
        println!(
            "💬 WhatsApp: {} account(s) configured",
            config.channels.whatsapp_native.len()
        );
        println!();
    }

    // AI Provider info
    println!(
        "🤖 AI Provider: {} ({})",
        config.default_provider,
        config.default_model.as_deref().unwrap_or("default model")
    );
    println!();

    // Quick tips
    println!("╭───────────────────────────────────────────────────────────────╮");
    println!("│ Quick Tips:                                                   │");
    println!("│ • Open Web UI in your browser to configure & chat             │");
    println!("│ • Message your Telegram bot to interact on the go             │");
    println!("│ • Press Ctrl+C to stop the agent                              │");
    println!("╰───────────────────────────────────────────────────────────────╯");
    println!();
}

pub fn heartbeat_thread(
    config: Arc<Config>,
    state: Arc<std::sync::Mutex<DaemonState>>,
    engine: HeartbeatEngine,
) {
    let state_path = state_file_path(&config.config_path);

    while !is_shutdown_requested() {
        // Write state file
        {
            let guard = state.lock().unwrap();
            if let Err(e) = write_state_file(&state_path, &guard) {
                warn!("Failed to write state file: {}", e);
            }
        }

        if let Err(e) = engine.tick(()) {
            warn!("Heartbeat tick failed: {}", e);
        }

        thread::sleep(Duration::from_secs(STATUS_FLUSH_SECONDS));
    }
}

pub fn scheduler_thread(
    config: Arc<Config>,
    state: Arc<std::sync::Mutex<DaemonState>>,
    scheduler: Arc<CronScheduler>,
) {
    let name = "scheduler";
    let poll_secs = config.reliability.scheduler_poll_secs; // Assuming this exists or using default

    while !is_shutdown_requested() {
        {
            let mut guard = state.lock().unwrap();
            guard.mark_running(name);
        }

        scheduler.tick();

        // Dynamic sleep to honor poll_secs
        thread::sleep(Duration::from_secs(poll_secs));
    }
}

pub fn inbound_dispatcher_thread(
    bus: Arc<Bus>,
    session_manager: Arc<SessionManager>,
    state: Arc<std::sync::Mutex<DaemonState>>,
) {
    let name = "inbound_dispatcher";
    info!("Inbound dispatcher thread started");

    // Create a multi-threaded Tokio runtime for concurrent processing
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build multi-thread runtime for inbound dispatcher");

    loop {
        if is_shutdown_requested() {
            break;
        }

        {
            let mut guard = state.lock().unwrap();
            guard.mark_running(name);
        }

        if let Some(msg) = bus.consume_inbound_timeout(Duration::from_millis(100)) {
            // Integrate Routing
            let mut input = crate::config_types::RouteInput {
                channel: msg.channel.clone(),
                account_id: String::new(), // To be resolved
                peer: None,
                parent_peer: None,
                guild_id: None,
                team_id: None,
                member_role_ids: Vec::new(),
            };

            // Parse metadata if available
            if let Some(meta_json) = &msg.metadata_json
                && let Ok(meta) =
                    serde_json::from_str::<crate::channel_adapters::InboundMetadata>(meta_json)
                {
                    input.account_id = meta.account_id.unwrap_or_default();
                    input.guild_id = meta.guild_id;
                    input.team_id = meta.team_id;
                    if let (Some(kind), Some(id)) = (meta.peer_kind, meta.peer_id) {
                        input.peer = Some(crate::config_types::PeerRef { kind, id });
                    }
                }

            if input.account_id.is_empty() {
                // Determine account_id from session_key if not in metadata (legacy logic)
                if let Some(idx) = msg.session_key.find(':') {
                    input.account_id = msg.session_key[0..idx].to_string();
                }
            }

            let route = crate::agent_routing::resolve_route_with_session(
                &input,
                &session_manager.get_config().bindings,
                &session_manager.get_config().agents,
                &session_manager.get_config().session,
            );

            // Process message in the local runtime
            let session_key = route.session_key.clone();
            let agent_id = route.agent_id.clone();
            let content = msg.content.clone();
            let channel = msg.channel.clone();
            let chat_id = msg.chat_id.clone();
            let sender_id = msg.sender_id.clone();
            let sm = session_manager.clone();
            let bus_clone = bus.clone();

            rt.spawn(async move {
                // Pass a callback to the agent that streams deltas back to the channel
                let cb_channel = channel.clone();
                let cb_chat_id = chat_id.clone();
                let cb_bus = bus_clone.clone();

                let acc_text = Arc::new(std::sync::Mutex::new(String::new()));
                let last_emit = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));

                let stream_cb: crate::providers::StreamCallback = Box::new(move |chunk| {
                    use crate::providers::StreamChunk;
                    if let StreamChunk::Delta(text) = chunk {
                        let mut acc = acc_text.lock().unwrap();
                        acc.push_str(&text);

                        let now = std::time::Instant::now();
                        let mut last = last_emit.lock().unwrap();

                        // Limit to 5 chunks per second (200ms) for smoother streaming
                        if now.duration_since(*last).as_millis() > 200 {
                            *last = now;
                            let outbound = bus::make_outbound_chunk(&cb_channel, &cb_chat_id, &acc);
                            let _ = cb_bus.publish_outbound(outbound);
                        }
                    }
                });

                let context = crate::tools::ToolContext {
                    channel: channel.clone(),
                    chat_id: chat_id.clone(),
                    sender_id: sender_id.clone(),
                    session_key: session_key.clone(),
                };

                match sm
                    .process_message_stream(&session_key, &agent_id, content, context, stream_cb)
                    .await
                {
                    Ok(response_text) => {
                        let outbound = bus::make_outbound(&channel, &chat_id, &response_text);
                        if let Err(e) = bus_clone.publish_outbound(outbound) {
                            error!("Failed to publish outbound message: {}", e);
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to process message for session {}: {}",
                            session_key, e
                        );
                        // FIXED: Send error back to user instead of silent failure
                        let error_msg = format!(
                            "⚠️ I encountered an error while processing your request: {}",
                            e
                        );
                        let outbound = bus::make_outbound(&channel, &chat_id, &error_msg);
                        let _ = bus_clone.publish_outbound(outbound);
                    }
                }
            });
        }
    }
    info!("Inbound dispatcher thread stopped");
}

pub fn init_bus() -> Arc<Bus> {
    Arc::new(Bus::new())
}

pub fn init_channels(
    config: &Config,
    _bus: Arc<Bus>,
) -> (ChannelRegistry, Vec<(PollingState, thread::JoinHandle<()>)>) {
    let mut registry = ChannelRegistry::new();
    let mut polling_threads = Vec::new();
    let mut runtime = ChannelRuntime;
    let mut seen_telegram_tokens = HashSet::new();

    // Initialize Telegram channels
    for tg_config in &config.channels.telegram {
        if tg_config.bot_token.is_empty() {
            warn!(
                "Skipping Telegram account {}: no bot token",
                tg_config.account_id
            );
            continue;
        }
        if !seen_telegram_tokens.insert(tg_config.bot_token.clone()) {
            warn!(
                "Skipping duplicate Telegram bot token for account {}",
                tg_config.account_id
            );
            continue;
        }

        info!(
            "Initializing Telegram channel for account: {}",
            tg_config.account_id
        );

        let channel = Arc::new(TelegramChannel::new(tg_config.clone()));
        registry.register(channel.clone());

        if let Some(descriptor) = find_polling_descriptor("telegram") {
            info!(
                "Starting Telegram polling for account: {}",
                tg_config.account_id
            );

            match (descriptor.spawn)((), config, &mut runtime, channel.clone()) {
                Ok(result) => {
                    if let (Some(state), Some(thread)) = (result.state, result.thread) {
                        polling_threads.push((state, thread));
                        info!(
                            "Telegram polling started for account: {}",
                            tg_config.account_id
                        );
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to start Telegram polling for account {}: {}",
                        tg_config.account_id, e
                    );
                }
            }
        }
    }

    // Initialize CLI channel if enabled
    if config.channels.cli {
        info!("Initializing CLI channel");
        let channel = Arc::new(crate::channels::cli::CliChannel::new(
            "cli_main".to_string(),
        ));
        registry.register(channel);
        // Note: CliChannel spawns its own thread for stdin, no polling thread needed here.
    }

    info!(
        "Channel initialization complete: {} channels, {} polling threads",
        registry.count(),
        polling_threads.len()
    );

    (registry, polling_threads)
}

pub fn start_outbound_dispatcher(
    bus: Arc<Bus>,
    registry: Arc<ChannelRegistry>,
    state: Arc<std::sync::Mutex<DaemonState>>,
) -> thread::JoinHandle<()> {
    let name = "outbound_dispatcher";
    thread::spawn(move || {
        {
            let mut guard = state.lock().unwrap();
            guard.mark_running(name);
        }
        run_outbound_dispatcher(bus, registry);
    })
}

/// Create the appropriate provider based on config, wrapped in ReliableProvider.
pub async fn build_tools(
    config: &Config,
    subagent_manager: Option<Arc<crate::subagent::SubagentManager>>,
    memory: Option<Arc<dyn crate::agent::memory_loader::Memory>>,
    cron: Option<Arc<crate::cron::CronScheduler>>,
) -> Vec<Arc<dyn crate::tools::Tool>> {
    let mut tools: Vec<Arc<dyn crate::tools::Tool>> = Vec::new();

    let _raw_memory_store: Option<Arc<dyn crate::memory::MemoryStore>> =
        match config.memory.backend.as_str() {
            "markdown" => Some(Arc::new(
                crate::memory::engines::markdown::MarkdownMemory::from_workspace(
                    &config.workspace_dir,
                ),
            )),
            "none" => None,
            _ => {
                let db_path = format!("{}/memory.db", config.workspace_dir);
                match crate::memory::sqlite::SqliteMemory::new(&db_path) {
                    Ok(m) => Some(Arc::new(m)),
                    Err(_) => None,
                }
            }
        };

    if let Some(mem) = memory {
        tools.push(Arc::new(crate::tools::memory_store::MemoryStoreTool {
            memory: mem.clone(),
        }));
        tools.push(Arc::new(crate::tools::memory_recall::MemoryRecallTool {
            memory: mem.clone(),
        }));
        tools.push(Arc::new(crate::tools::memory_forget::MemoryForgetTool {
            memory: mem.clone(),
        }));
        tools.push(Arc::new(crate::tools::memory_list::MemoryListTool {
            memory: mem.clone(),
        }));
    }

    tools.push(Arc::new(crate::tools::file_read::FileReadTool {
        workspace_dir: config.workspace_dir.clone(),
        allowed_paths: vec![config.workspace_dir.clone()],
        max_file_size: 10 * 1024 * 1024,
    }));
    tools.push(Arc::new(crate::tools::file_write::FileWriteTool {
        workspace_dir: config.workspace_dir.clone(),
        allowed_paths: vec![config.workspace_dir.clone()],
    }));
    tools.push(Arc::new(crate::tools::file_edit::FileEditTool {
        workspace_dir: config.workspace_dir.clone(),
        allowed_paths: vec![config.workspace_dir.clone()],
        max_file_size: 10 * 1024 * 1024,
    }));
    tools.push(Arc::new(crate::tools::file_append::FileAppendTool {
        workspace_dir: config.workspace_dir.clone(),
        allowed_paths: vec![config.workspace_dir.clone()],
        max_file_size: 10 * 1024 * 1024,
    }));

    tools.push(Arc::new(crate::tools::shell::ShellTool {
        workspace_dir: config.workspace_dir.clone(),
        allowed_paths: vec![config.workspace_dir.clone()],
        timeout_ns: 30_000_000_000,
        max_output_bytes: 1024 * 1024,
    }));

    tools.push(Arc::new(crate::tools::git::GitTool {
        workspace_dir: config.workspace_dir.clone(),
        allowed_paths: vec![config.workspace_dir.clone()],
    }));

    tools.push(Arc::new(crate::tools::http_request::HttpRequestTool {
        max_response_size: config.http_request.max_response_size as usize,
        timeout_secs: config.http_request.timeout_secs,
        allowed_domains: config.http_request.allowed_domains.clone(),
    }));

    if config.composio.enabled
        && let Some(api_key) = &config.composio.api_key {
            tools.push(Arc::new(crate::tools::composio::ComposioTool {
                api_key: api_key.clone(),
                entity_id: config.composio.entity_id.clone(),
            }));
        }

    tools.push(Arc::new(crate::tools::browser::BrowserTool::new(
        config.workspace_dir.clone(),
    )));

    let mut search_tool = crate::tools::web_search::WebSearchTool::default();
    let req_config = &config.http_request;
    search_tool.provider = req_config.search_provider.clone();
    search_tool.api_key = req_config.brave_search_api_key.clone();
    tools.push(Arc::new(search_tool));

    tools.push(Arc::new(crate::tools::web_fetch::WebFetchTool {
        default_max_chars: 50_000,
    }));

    tools.push(Arc::new(crate::tools::skill_search::SkillSearchTool {
        workspace_dir: config.workspace_dir.clone(),
    }));
    tools.push(Arc::new(crate::tools::skill_install::SkillInstallTool {
        workspace_dir: config.workspace_dir.clone(),
    }));
    tools.push(Arc::new(crate::tools::skill_list::SkillListTool {
        workspace_dir: config.workspace_dir.clone(),
        builtin_dir: config.workspace_dir.clone(),
    }));
    tools.push(Arc::new(
        crate::tools::skill_uninstall::SkillUninstallTool {
            workspace_dir: config.workspace_dir.clone(),
        },
    ));

    tools.push(Arc::new(crate::tools::browser_open::BrowserOpenTool {
        allowed_domains: config.http_request.allowed_domains.clone(),
    }));

    tools.push(Arc::new(crate::tools::schedule::ScheduleTool {}));
    if let Some(cr) = cron {
        tools.push(Arc::new(crate::tools::cron_add::CronAddTool {
            cron: cr.clone(),
        }));
        tools.push(Arc::new(crate::tools::cron_list::CronListTool {
            cron: cr.clone(),
        }));
        tools.push(Arc::new(crate::tools::cron_remove::CronRemoveTool {
            cron: cr.clone(),
        }));
        tools.push(Arc::new(crate::tools::cron_run::CronRunTool {
            cron: cr.clone(),
        }));
        tools.push(Arc::new(crate::tools::cron_runs::CronRunsTool {
            cron: cr.clone(),
        }));
        tools.push(Arc::new(crate::tools::cron_update::CronUpdateTool {
            cron: cr.clone(),
        }));
    }

    tools.push(Arc::new(crate::tools::image::ImageInfoTool {}));
    tools.push(Arc::new(crate::tools::screenshot::ScreenshotTool {}));
    tools.push(Arc::new(crate::tools::pushover::PushoverTool {
        workspace_dir: config.workspace_dir.clone(),
    }));

    tools.push(Arc::new(crate::tools::hardware_info::HardwareInfoTool {}));
    tools.push(Arc::new(crate::tools::hardware_memory::HardwareMemoryTool {
        boards: Vec::new(),
    }));
    tools.push(Arc::new(crate::tools::i2c::I2cTool {}));
    tools.push(Arc::new(crate::tools::spi::SpiTool {}));

    if let Ok(mcp_tools) = crate::mcp::init_mcp_tools(&config.mcp_servers).await {
        for tool in mcp_tools {
            tools.push(tool);
        }
    }

    if let Ok(skills) = crate::skills::list_skills(Path::new(&config.workspace_dir)) {
        for skill in skills {
            if skill.enabled && skill.available {
                let skill_path = std::path::PathBuf::from(&skill.path);
                for tool_def in skill.tools {
                    tools.push(Arc::new(crate::tools::skill_tool::DynamicSkillTool {
                        definition: tool_def,
                        skill_path: skill_path.clone(),
                    }));
                }
            }
        }
    }

    if let Some(sm) = subagent_manager {
        tools.push(Arc::new(crate::tools::delegate::DelegateTool {}));
        tools.push(Arc::new(crate::tools::spawn::SpawnTool {
            subagent_manager: sm,
        }));
        tools.push(Arc::new(crate::tools::message::MessageTool::new()));
    }

    tools
}

pub fn create_provider(config: &Config) -> Arc<dyn Provider> {
    let provider_name = &config.default_provider;
    factory::create_with_fallbacks(provider_name, config)
}

pub async fn run_daemon(config: Config) -> Result<()> {
    info!("Initializing OpenPaw daemon...");

    let config = Arc::new(config);

    // Initialize the global bus first (before any threads that use it)
    // All threads will clone this to share the same underlying channels
    let global_bus = bus::init_global_bus().clone();
    let bus = Arc::new(global_bus);
    let daemon_state = Arc::new(std::sync::Mutex::new(DaemonState::default()));
    {
        let mut ds = daemon_state.lock().unwrap();
        ds.gateway_host = config.gateway.host.clone();
        ds.gateway_port = config.gateway.port;
        ds.add_component("gateway");
        ds.add_component("heartbeat");
        ds.add_component("scheduler");
        ds.add_component("inbound_dispatcher");
        ds.add_component("outbound_dispatcher");
        ds.started = true;
    }

    // Initialize Provider based on config
    let provider: Arc<dyn Provider> = create_provider(&config);

    // Initialize Memory — read backend from config
    let memory: Option<Arc<dyn crate::agent::memory_loader::Memory>> = {
        match config.memory.backend.as_str() {
            "markdown" => {
                let m = crate::memory::engines::markdown::MarkdownMemory::from_workspace(
                    &config.workspace_dir,
                );
                info!("Memory backend: markdown (MEMORY.md)");
                Some(Arc::new(crate::agent::memory_loader::MemoryAdapter {
                    inner: Arc::new(m),
                }))
            }
            "none" => {
                info!("Memory backend: none (ephemeral)");
                None
            }
            _ => {
                // Default: sqlite
                let db_path = format!("{}/memory.db", config.workspace_dir);
                match crate::memory::sqlite::SqliteMemory::new(&db_path) {
                    Ok(mut m) => {
                        info!("Memory backend: sqlite ({})", db_path);

                        // Attach embedder if API key is available
                        let provider_name = &config.default_provider;
                        if let Some(p_cfg) = config
                            .models
                            .as_ref()
                            .and_then(|m| m.providers.get(provider_name))
                        {
                            let provider_key = p_cfg.api_key.trim();
                            let has_real_provider_key = !(provider_key.is_empty()
                                || provider_name == "gemini"
                                    && provider_key.eq_ignore_ascii_case("cli_oauth"));

                            if has_real_provider_key {
                                if provider_name == "openai" {
                                    m = m.with_embedder(Arc::new(
                                        crate::memory::embeddings::OpenAiEmbedder::new(
                                            &p_cfg.api_key,
                                        ),
                                    ));
                                    info!("Attached OpenAI embedder to memory");
                                } else if provider_name == "gemini" {
                                    m = m.with_embedder(Arc::new(
                                        crate::memory::embeddings::GeminiEmbedder::new(
                                            &p_cfg.api_key,
                                        ),
                                    ));
                                    info!("Attached Gemini embedder to memory");
                                } else if provider_name == "huggingface" || provider_name == "hf" {
                                    let model = config
                                        .memory
                                        .embedding_model
                                        .clone()
                                        .unwrap_or("Qwen/Qwen3-Embedding-0.6B".to_string());
                                    m = m.with_embedder(Arc::new(
                                        crate::memory::embeddings::HuggingFaceEmbedder::new(
                                            &p_cfg.api_key,
                                            &model,
                                        ),
                                    ));
                                    info!("Attached Hugging Face embedder ({}) to memory", model);
                                }
                            } else if provider_name == "gemini"
                                && provider_key.eq_ignore_ascii_case("cli_oauth")
                            {
                                info!(
                                    "Skipping Gemini embedder: CLI OAuth mode selected and embeddings require an API key"
                                );
                            }
                        }

                        Some(Arc::new(crate::agent::memory_loader::MemoryAdapter {
                            inner: Arc::new(m),
                        }))
                    }
                    Err(e) => {
                        warn!("Failed to init SQLite memory: {} — using noop", e);
                        Some(Arc::new(crate::agent::memory_loader::NoopMemory))
                    }
                }
            }
        }
    };

    // Initialize Subagent Manager
    let subagent_manager = Arc::new(crate::subagent::SubagentManager::new(
        bus.clone(),
        crate::subagent::SubagentConfig::default(),
        (*config).clone(),
    ));

    // Start Scheduler
    let bus_handle = bus.clone();
    let scheduler = Arc::new(CronScheduler::init((), &config, &bus_handle));

    // Initialize Tools
    let tools = build_tools(
        &config,
        Some(subagent_manager.clone()),
        memory.clone(),
        Some(scheduler.clone()),
    )
    .await;

    let session_manager = Arc::new(SessionManager::new(config.clone(), provider, tools, memory));

    // Start Inbound Dispatcher
    let bus_clone = bus.clone();
    let sm_clone = session_manager.clone();
    let ds_clone = daemon_state.clone();
    thread::spawn(move || inbound_dispatcher_thread(bus_clone, sm_clone, ds_clone));

    // Finish Subagent Manager initialization (inject session manager to break circularity)
    subagent_manager.set_session_manager(session_manager.clone());

    // Initialize Channels
    let mut _bridge_manager = None;
    for wa_cfg in &config.channels.whatsapp_native {
        if wa_cfg.auto_start {
            let bridge_dir = if let Some(dir) = &wa_cfg.bridge_dir {
                dir.clone()
            } else {
                format!(
                    "{}/src/channels/whatsapp_native_bridge",
                    config.workspace_dir
                )
            };

            let manager = crate::channels::whatsapp_bridge_manager::BridgeManager::new(bridge_dir);
            match manager.start() {
                Ok(_) => {
                    info!("WhatsApp Bridge started automatically.");
                    _bridge_manager = Some(manager);
                    break;
                }
                Err(e) => warn!("Failed to auto-start WhatsApp Bridge: {}", e),
            }
        }
    }

    let (registry, polling_threads) = init_channels(&config, bus.clone());
    let registry = Arc::new(registry);

    // Start Outbound Dispatcher
    let ds_clone = daemon_state.clone();
    let dispatcher_handle = start_outbound_dispatcher(bus.clone(), registry.clone(), ds_clone);

    // Start Heartbeat
    let heartbeat_engine = HeartbeatEngine::init(true, 30, &config.workspace_dir, None);
    let ds_clone = daemon_state.clone();
    let config_clone = config.clone();
    thread::spawn(move || heartbeat_thread(config_clone, ds_clone, heartbeat_engine));

    let config_clone2 = config.clone();
    let sched_clone = scheduler.clone();
    let ds_clone = daemon_state.clone();
    thread::spawn(move || scheduler_thread(config_clone2, ds_clone, sched_clone));

    // Start Gateway (Web UI) supervised
    let config_clone = config.clone();
    let ds_clone = daemon_state.clone();
    tokio::spawn(async move {
        gateway_thread(config_clone, ds_clone).await;
    });

    // Print startup info
    print_startup_info(&config);

    // Run health check
    let health = registry.health_check_all();
    info!(
        "Channel health check: {}/{} healthy",
        health.healthy, health.total
    );

    info!("Daemon running. Press Ctrl+C to stop.");

    // Ctrl+C / SIGINT handler — calls request_shutdown() so the loop below exits cleanly
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let sf_clone = shutdown_flag.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            sf_clone.store(true, Ordering::Release);
            request_shutdown();
        }
    });

    // Wait for shutdown signal
    loop {
        if is_shutdown_requested() || shutdown_flag.load(Ordering::Acquire) {
            info!("Shutdown requested, stopping daemon...");
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }

    // Stop polling threads
    for (state, handle) in polling_threads {
        channel_loop::stop_polling(&state);
        let _ = handle.join();
    }

    let _ = dispatcher_handle.join();

    info!("Daemon stopped");
    Ok(())
}
