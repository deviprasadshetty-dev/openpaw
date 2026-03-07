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
            });
        }
    }

    pub fn mark_error(&mut self, name: &str, err_msg: &str) {
        if let Some(comp) = self.components.iter_mut().find(|c| c.name == name) {
            comp.running = false;
            comp.last_error = Some(err_msg.to_string());
            comp.restart_count += 1;
        }
    }

    pub fn mark_running(&mut self, name: &str) {
        if let Some(comp) = self.components.iter_mut().find(|c| c.name == name) {
            comp.running = true;
            comp.last_error = None;
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

pub async fn gateway_thread(config: Config) {
    if let Err(e) = gateway::serve(config).await {
        eprintln!("Gateway error: {}", e);
    }
}

pub fn heartbeat_thread(
    _config: Config,
    _state: Arc<std::sync::Mutex<DaemonState>>,
    engine: HeartbeatEngine,
) {
    while !is_shutdown_requested() {
        if let Err(e) = engine.tick(()) {
            warn!("Heartbeat tick failed: {}", e);
        }
        thread::sleep(Duration::from_secs(STATUS_FLUSH_SECONDS));
    }
}

pub fn scheduler_thread(_config: Config, scheduler: Arc<CronScheduler>) {
    while !is_shutdown_requested() {
        scheduler.tick();
        thread::sleep(Duration::from_secs(60));
    }
}

pub fn inbound_dispatcher_thread(bus: Arc<Bus>, session_manager: Arc<SessionManager>) {
    info!("Inbound dispatcher thread started");

    // Create a local Tokio runtime for executing async session logic
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build local runtime for inbound dispatcher");

    loop {
        if is_shutdown_requested() {
            break;
        }

        if let Some(msg) = bus.consume_inbound_timeout(Duration::from_millis(100)) {
            // Process message in the local runtime
            let session_key = msg.session_key.clone();
            let content = msg.content.clone();
            let channel = msg.channel.clone();
            let chat_id = msg.chat_id.clone();
            let sm = session_manager.clone();
            let bus_clone = bus.clone();

            rt.block_on(async move {
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

                        // Limit to 1 chunk per second
                        if now.duration_since(*last).as_millis() > 1000 {
                            *last = now;
                            let outbound = bus::make_outbound_chunk(&cb_channel, &cb_chat_id, &acc);
                            let _ = cb_bus.publish_outbound(outbound);
                        }
                    }
                });

                match sm
                    .process_message_stream(&session_key, content, stream_cb)
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
                        // Optionally send error back to user
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
) -> thread::JoinHandle<()> {
    run_outbound_dispatcher(bus, registry)
}

/// Create the appropriate provider based on config, wrapped in ReliableProvider.
fn create_provider(config: &Config) -> Arc<dyn Provider> {
    let provider_name = &config.default_provider;
    factory::create_with_fallbacks(provider_name, config)
}

pub async fn run_daemon(config: Config) -> Result<()> {
    info!("Initializing OpenPaw daemon...");

    // Initialize the global bus first (before any threads that use it)
    // All threads will clone this to share the same underlying channels
    let global_bus = bus::init_global_bus().clone();
    let bus = Arc::new(global_bus);
    let daemon_state = Arc::new(std::sync::Mutex::new(DaemonState::default()));

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
                            let has_real_provider_key = !provider_key.is_empty()
                                && !(provider_name == "gemini"
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

    // Initialize Tools
    let mut tools: Vec<Arc<dyn crate::tools::Tool>> = Vec::new();

    // Memory tools — wire to the raw MemoryStore backend so the agent can
    // explicitly store, recall, list, and forget memories at runtime.
    let raw_memory_store: Option<Arc<dyn crate::memory::MemoryStore>> =
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

    if let Some(ref ms) = raw_memory_store {
        tools.push(Arc::new(crate::tools::memory_store::MemoryStoreTool {
            memory: ms.clone(),
        }));
        tools.push(Arc::new(crate::tools::memory_recall::MemoryRecallTool {
            memory: ms.clone(),
        }));
        tools.push(Arc::new(crate::tools::memory_forget::MemoryForgetTool {
            memory: ms.clone(),
        }));
        tools.push(Arc::new(crate::tools::memory_list::MemoryListTool {
            memory: ms.clone(),
        }));
    }

    // File Tools
    tools.push(Arc::new(crate::tools::file_read::FileReadTool {
        workspace_dir: config.workspace_dir.clone(),
        allowed_paths: vec![config.workspace_dir.clone()],
        max_file_size: 10 * 1024 * 1024, // 10MB
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

    // Shell Tool
    tools.push(Arc::new(crate::tools::shell::ShellTool {
        workspace_dir: config.workspace_dir.clone(),
        allowed_paths: vec![config.workspace_dir.clone()],
        timeout_ns: 30_000_000_000, // 30s
        max_output_bytes: 1024 * 1024,
    }));

    // Git Tool
    tools.push(Arc::new(crate::tools::git::GitTool {
        workspace_dir: config.workspace_dir.clone(),
        allowed_paths: vec![config.workspace_dir.clone()],
    }));

    // HTTP Request Tool
    tools.push(Arc::new(crate::tools::http_request::HttpRequestTool {
        max_response_size: config.http_request.max_response_size as usize,
        timeout_secs: config.http_request.timeout_secs,
        allowed_domains: config.http_request.allowed_domains.clone(),
    }));

    // Composio Tool
    if config.composio.enabled {
        if let Some(api_key) = &config.composio.api_key {
            tools.push(Arc::new(crate::tools::composio::ComposioTool {
                api_key: api_key.clone(),
                entity_id: config.composio.entity_id.clone(),
            }));
        }
    }

    // Browser Tool — auto-detects Chrome/Edge/Brave, uses dedicated openpaw profile
    tools.push(Arc::new(crate::tools::browser::BrowserTool::new(
        config.workspace_dir.clone(),
    )));

    // Web Search Tool
    let mut search_tool = crate::tools::web_search::WebSearchTool::default();
    let req_config = &config.http_request;
    search_tool.provider = req_config.search_provider.clone();
    search_tool.api_key = req_config.brave_search_api_key.clone();
    tools.push(Arc::new(search_tool));

    // Web Fetch Tool
    tools.push(Arc::new(crate::tools::web_fetch::WebFetchTool {
        default_max_chars: 50_000,
    }));

    // Skill Tools — search GitHub for skills and install them into workspace/skills/
    tools.push(Arc::new(crate::tools::skill_search::SkillSearchTool {
        workspace_dir: config.workspace_dir.clone(),
    }));
    tools.push(Arc::new(crate::tools::skill_install::SkillInstallTool {
        workspace_dir: config.workspace_dir.clone(),
    }));
    tools.push(Arc::new(crate::tools::skill_list::SkillListTool {
        workspace_dir: config.workspace_dir.clone(),
        builtin_dir: config.workspace_dir.clone(), // Assuming built-ins are also in workspace for now or reachable
    }));
    tools.push(Arc::new(
        crate::tools::skill_uninstall::SkillUninstallTool {
            workspace_dir: config.workspace_dir.clone(),
        },
    ));

    // Browser Open Tool
    tools.push(Arc::new(crate::tools::browser_open::BrowserOpenTool {
        allowed_domains: config.http_request.allowed_domains.clone(),
    }));

    // Cron/Schedule Tools
    tools.push(Arc::new(crate::tools::schedule::ScheduleTool {}));
    tools.push(Arc::new(crate::tools::cron_add::CronAddTool {}));
    tools.push(Arc::new(crate::tools::cron_list::CronListTool {}));
    tools.push(Arc::new(crate::tools::cron_remove::CronRemoveTool {}));
    tools.push(Arc::new(crate::tools::cron_run::CronRunTool {}));
    tools.push(Arc::new(crate::tools::cron_runs::CronRunsTool {}));
    tools.push(Arc::new(crate::tools::cron_update::CronUpdateTool {}));

    // Utility Tools
    tools.push(Arc::new(crate::tools::image::ImageInfoTool {}));
    tools.push(Arc::new(crate::tools::screenshot::ScreenshotTool {
        workspace_dir: config.workspace_dir.clone(),
    }));
    tools.push(Arc::new(crate::tools::pushover::PushoverTool {
        workspace_dir: config.workspace_dir.clone(),
    }));
    tools.push(Arc::new(crate::tools::delegate::DelegateTool {}));
    // Initialize Subagent Manager
    let subagent_manager = Arc::new(crate::subagent::SubagentManager::new(
        bus.clone(),
        crate::subagent::SubagentConfig::default(),
    ));

    tools.push(Arc::new(crate::tools::spawn::SpawnTool {
        subagent_manager: subagent_manager.clone(),
    }));
    tools.push(Arc::new(crate::tools::message::MessageTool::new()));

    // Hardware Tools
    tools.push(Arc::new(
        crate::tools::hardware_info::HardwareBoardInfoTool { boards: Vec::new() },
    ));
    tools.push(Arc::new(
        crate::tools::hardware_memory::HardwareMemoryTool { boards: Vec::new() },
    ));
    tools.push(Arc::new(crate::tools::i2c::I2cTool {}));
    tools.push(Arc::new(crate::tools::spi::SpiTool {}));

    // Dynamic Skill Tools — tools defined in SKILL.toml/skill.json
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

    let session_manager = Arc::new(SessionManager::new(
        provider,
        tools,
        memory,
        config.default_model.clone().unwrap_or("gpt-4o".to_string()),
        config.workspace_dir.clone(),
        if config.config_path.is_empty() {
            None
        } else {
            Some(config.config_path.clone())
        },
    ));

    // Start Inbound Dispatcher
    let bus_clone = bus.clone();
    let sm_clone = session_manager.clone();
    thread::spawn(move || inbound_dispatcher_thread(bus_clone, sm_clone));

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
    let dispatcher_handle = start_outbound_dispatcher(bus.clone(), registry.clone());

    // Start Heartbeat
    let heartbeat_engine = HeartbeatEngine::init(true, 30, &config.workspace_dir, None);
    let ds_clone = daemon_state.clone();
    let config_clone = config.clone();
    thread::spawn(move || heartbeat_thread(config_clone, ds_clone, heartbeat_engine));

    // Start Scheduler
    let scheduler = Arc::new(CronScheduler::init((), &config, &bus));
    let config_clone2 = config.clone();
    let sched_clone = scheduler.clone();
    thread::spawn(move || scheduler_thread(config_clone2, sched_clone));

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
