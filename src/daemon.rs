use crate::bus::{self, Bus, InboundMessage};
use crate::channel_adapters::find_polling_descriptor;
use crate::channel_loop::{self, ChannelRuntime, PollingState};
use crate::channels::dispatch::{run_outbound_dispatcher, ChannelRegistry};
use crate::channels::telegram::TelegramChannel;
use crate::config::Config;
use crate::cron::CronScheduler;
use crate::gateway;
use crate::heartbeat::HeartbeatEngine;
use crate::providers::openai::OpenAiCompatibleProvider;
use crate::session::SessionManager;
use crate::agent::memory_loader::Memory;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
    config: Config,
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

pub fn scheduler_thread(_config: Config, _scheduler: Arc<CronScheduler>) {
    while !is_shutdown_requested() {
        // Scheduler tick logic (stub for now, needs full implementation)
        // scheduler.tick();
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
                match sm.process_message(&session_key, content).await {
                    Ok(response_text) => {
                        let outbound = bus::make_outbound(&channel, &chat_id, &response_text);
                        if let Err(e) = bus_clone.publish_outbound(outbound) {
                            error!("Failed to publish outbound message: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("Failed to process message for session {}: {}", session_key, e);
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
    bus: Arc<Bus>,
) -> (ChannelRegistry, Vec<(PollingState, thread::JoinHandle<()>)>) {
    let mut registry = ChannelRegistry::new();
    let mut polling_threads = Vec::new();
    let mut runtime = ChannelRuntime;

    // Initialize Telegram channels
    for tg_config in &config.channels.telegram {
        if tg_config.bot_token.is_empty() {
            warn!("Skipping Telegram account {}: no bot token", tg_config.account_id);
            continue;
        }

        info!(
            "Initializing Telegram channel for account: {}",
            tg_config.account_id
        );

        let channel = Arc::new(TelegramChannel::new(tg_config.clone()));
        registry.register(channel.clone());

        if let Some(descriptor) = find_polling_descriptor("telegram") {
            info!("Starting Telegram polling for account: {}", tg_config.account_id);

            match (descriptor.spawn)(
                (),
                config,
                &mut runtime,
                channel.clone(),
            ) {
                Ok(result) => {
                    if let (Some(state), Some(thread)) = (result.state, result.thread) {
                        polling_threads.push((state, thread));
                        info!("Telegram polling started for account: {}", tg_config.account_id);
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

pub async fn run_daemon(config: Config) -> Result<()> {
    info!("Initializing OpenPaw daemon...");

    let bus = init_bus();
    let daemon_state = Arc::new(std::sync::Mutex::new(DaemonState::default()));

    // Initialize SessionManager
    let provider = Arc::new(OpenAiCompatibleProvider::new(
        "default",
        &config.models.as_ref().map(|m| m.providers.get(&config.default_provider).map(|p| p.base_url.clone().unwrap_or_default()).unwrap_or_default()).unwrap_or_default(),
        &config.models.as_ref().map(|m| m.providers.get(&config.default_provider).map(|p| p.api_key.clone()).unwrap_or_default()).unwrap_or_default(),
    ));

    // Initialize Memory
    let db_path = std::path::Path::new(&config.workspace_dir).join("openpaw.db");
    let memory: Option<Arc<dyn crate::agent::memory_loader::Memory>> = match crate::memory::sqlite::SqliteMemory::new(db_path.to_str().unwrap()) {
        Ok(m) => Some(Arc::new(m) as Arc<dyn crate::agent::memory_loader::Memory>),
        Err(e) => {
            error!("Failed to init SQLite memory: {}", e);
            None
        }
    };

    // Initialize Tools
    let mut tools: Vec<Arc<dyn crate::tools::Tool>> = Vec::new();
    
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

    // Web Search Tool
    let mut search_tool = crate::tools::web_search::WebSearchTool::default();
    let req_config = &config.http_request;
    search_tool.provider = req_config.search_provider.clone();
    tools.push(Arc::new(search_tool));

    let session_manager = Arc::new(SessionManager::new(
        provider,
        tools,
        memory,
        config.default_model.clone().unwrap_or("gpt-4o".to_string()),
        config.workspace_dir.clone(),
    ));

    // Start Inbound Dispatcher
    let bus_clone = bus.clone();
    let sm_clone = session_manager.clone();
    thread::spawn(move || inbound_dispatcher_thread(bus_clone, sm_clone));

    // Initialize Channels
    let (registry, polling_threads) = init_channels(&config, bus.clone());
    let registry = Arc::new(registry);

    // Start Outbound Dispatcher
    let dispatcher_handle = start_outbound_dispatcher(bus.clone(), registry.clone());

    // Start Heartbeat
    let heartbeat_engine = HeartbeatEngine::init(
        true,
        30,
        &config.workspace_dir,
        None,
    );
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

    // Wait for shutdown signal
    loop {
        if is_shutdown_requested() {
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
