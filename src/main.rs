pub mod agent;
pub mod agent_routing;
pub mod auth;
pub mod build_options;
pub mod bus;
pub mod capabilities;
pub mod channel_adapters;
pub mod channel_catalog;
pub mod channel_loop;
pub mod channel_manager;
pub mod channels;
mod config;
pub mod config_mutator;
pub mod config_parse;
pub mod config_types;
pub mod cost;
pub mod cron;
pub mod daemon;
pub mod doctor;
mod gateway;
pub mod hardware;
pub mod health;
pub mod heartbeat;
pub mod http_util;
pub mod identity;
pub mod integrations;
pub mod interactions;
pub mod json_util;
pub mod main_wasi;
pub mod mcp;
pub mod memory;
pub mod migration;
pub mod model_router;
pub mod multimodal;
pub mod net_security;
pub mod observability;
pub mod onboard;
pub mod peripherals;
pub mod platform;
pub mod portable_atomic;
pub mod porting;
pub mod providers;
pub mod rag;
pub mod runtime;
pub mod service;
pub mod session;
pub mod skillforge;
pub mod skills;
pub mod sse_client;
pub mod state;
pub mod status;
pub mod streaming;
pub mod subagent;
pub mod tools;
pub mod tunnel;
pub mod update;
pub mod util;
pub mod version;
pub mod voice;
pub mod websocket;
pub mod workspace_templates;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, level_filters::LevelFilter};
use tracing_subscriber::EnvFilter;

use crate::config_types::TelegramConfig;

#[derive(Parser, Debug)]
#[command(
    name = "openpaw",
    author,
    version,
    about = "OpenPaw — lightweight local AI agent",
    long_about = concat!(
        "\n",
        "  OpenPaw is a lightweight, privacy-focused AI agent that runs entirely\n",
        "  on your machine. Connect LLMs from Gemini, OpenAI, Anthropic, or OpenRouter.\n",
        "  Supports voice transcription (Groq Whisper), Telegram, MCP tools, skills,\n",
        "  and persistent memory (SQLite / Markdown / LRU).\n",
        "\n",
        "  Quick start:\n",
        "    openpaw onboard          # interactive setup wizard\n",
        "    openpaw agent            # start interactive agent\n",
        "    openpaw agent -m \"hi\"   # one-shot message\n",
    )
)]
struct Args {
    #[arg(short, long)]
    config: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Onboard a new workspace
    Onboard {
        #[arg(short, long, default_value = ".")]
        dir: String,
    },
    /// Start the gateway server
    Gateway,
    /// Start the agent daemon with all configured channels
    Agent {
        /// Optional message to send (one-shot mode)
        #[arg(short, long)]
        message: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();

    info!("Starting OpenPaw...");

    let args = Args::parse();

    let config_path = args.config.clone().or_else(|| {
        if std::path::Path::new("config.json").exists() {
            Some("config.json".to_string())
        } else {
            None
        }
    });

    let mut config = if let Some(path) = config_path {
        info!("Loading config from {}", path);
        let content = std::fs::read_to_string(&path)?;
        let mut cfg: config::Config = serde_json::from_str(&content)?;
        cfg.config_path = path;
        // Set workspace dir to current dir if not specified
        if cfg.workspace_dir.is_empty() {
            cfg.workspace_dir = ".".to_string();
        }
        cfg
    } else {
        info!("No config file found, using defaults");
        config::Config {
            default_temperature: Some(0.7),
            models: None,
            gateway: config::GatewayConfig::default(),
            channels: Default::default(),
            memory: Default::default(),
            http_request: Default::default(),
            browser: Default::default(),
            composio: Default::default(),
            hardware: Default::default(),
            pushover: Default::default(),
            mcp_servers: Default::default(),
            agents: Vec::new(),
            config_path: String::new(),
            workspace_dir: ".".to_string(),
            default_model: None,
            default_provider: "openai".to_string(),
        }
    };

    match &args.command {
        Some(Commands::Onboard { dir }) => {
            onboard::interactive_onboard(dir)?;
            return Ok(());
        }
        None => {
            // No command — print banner + quick help, then start daemon
            print!("{}", onboard::BANNER);
            println!("  No command specified. Starting agent daemon...");
            println!("  Tip: run `openpaw --help` to see all commands.\n");
            gateway::serve(config).await?;
        }
        Some(Commands::Gateway) => {
            gateway::serve(config).await?;
        }
        Some(Commands::Agent { message }) => {
            // Add Telegram config from environment variable if available (override/append)
            if let Ok(bot_token) = std::env::var("TELEGRAM_BOT_TOKEN") {
                let already_configured = config
                    .channels
                    .telegram
                    .iter()
                    .any(|c| c.bot_token == bot_token);

                if already_configured {
                    info!(
                        "TELEGRAM_BOT_TOKEN already present in config; skipping duplicate env Telegram channel"
                    );
                } else {
                    let allow_from = std::env::var("TELEGRAM_ALLOW_FROM")
                        .unwrap_or_else(|_| "*".to_string())
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .collect();

                    config.channels.telegram.push(TelegramConfig {
                        account_id: "env_main".to_string(),
                        bot_token,
                        allow_from,
                        group_allow_from: vec![],
                        group_policy: "allowlist".to_string(),
                        reply_in_private: true,
                        proxy: std::env::var("TELEGRAM_PROXY").ok(),
                    });

                    info!("Telegram bot configured from environment");
                }
            }

            if let Some(msg) = message {
                // One-shot mode - send a message and get response
                info!("One-shot message: {}", msg);
                run_one_shot_message(config, msg.to_string()).await?;
            } else {
                // Run the daemon with all configured channels
                daemon::run_daemon(config).await?;
            }
        }
    }

    Ok(())
}

async fn run_one_shot_message(config: crate::config::Config, message: String) -> Result<()> {
    use crate::agent::Agent;
    use crate::daemon::create_provider;
    use crate::tools::root::ToolContext;

    // Initialize provider
    let provider = create_provider(&config);

    // Get model name
    let model_name =
        config
            .default_model
            .clone()
            .unwrap_or_else(|| match config.default_provider.as_str() {
                "gemini" => "gemini-2.5-flash".to_string(),
                "openai" => "gpt-4o".to_string(),
                "anthropic" => "claude-3-5-sonnet-20241022".to_string(),
                _ => "gpt-4o".to_string(),
            });

    // Create agent with empty tools (one-shot mode doesn't need tools for now)
    let mut agent = Agent::new(
        provider,
        vec![], // Empty tools for one-shot
        model_name,
        config.workspace_dir,
    );

    // Create tool context (dummy values for CLI)
    let context = ToolContext {
        channel: "cli".to_string(),
        sender_id: "cli_user".to_string(),
        chat_id: "cli_chat".to_string(),
        session_key: "cli_session".to_string(),
    };

    // Run the agent turn
    println!("\n🤖 OpenPaw: Thinking...\n");
    let response = agent.turn(message, &context).await?;

    println!("\n🤖 OpenPaw: {}\n", response);

    Ok(())
}
