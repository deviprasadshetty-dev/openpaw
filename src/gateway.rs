use anyhow::Result;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tower_http::services::ServeDir;
use tracing::info;

use crate::config::Config;

#[derive(Clone)]
struct AppState {
    config: Arc<RwLock<Config>>,
}

pub async fn serve(config: Config) -> Result<()> {
    let state = AppState {
        config: Arc::new(RwLock::new(config.clone())),
    };

    let app = Router::new()
        // API routes
        .route("/api/health", get(health_check))
        .route("/api/config", get(get_config))
        .route("/api/config", post(save_config))
        .route("/api/chat", post(chat_handler))
        .route("/whatsapp/webhook", post(whatsapp_webhook_handler))
        .route("/ws/chat", get(websocket_handler))
        // Static files (must be first to catch all static routes)
        .fallback_service(ServeDir::new("static"))
        .with_state(state);

    let local_bind = if config.gateway.allow_public_bind.unwrap_or(false) {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };

    let bind_addr = format!("{}:{}", local_bind, config.gateway.port);
    let addr: SocketAddr = bind_addr.parse()?;

    info!("Gateway listening on {}", addr);
    info!("Web UI available at http://{}", addr);

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_check() -> &'static str {
    "ok"
}

// Config API
async fn get_config(State(state): State<AppState>) -> Json<Config> {
    let config = state.config.read().await;
    Json(config.clone())
}

#[derive(Debug, Deserialize)]
struct SaveConfigRequest {
    default_provider: Option<String>,
    default_model: Option<String>,
    models: Option<ModelsConfigInput>,
    memory: Option<MemoryConfigInput>,
    channels: Option<ChannelsConfigInput>,
    http_request: Option<HttpRequestConfigInput>,
    browser: Option<BrowserConfigInput>,
    composio: Option<ComposioConfigInput>,
}

#[derive(Debug, Deserialize)]
struct ModelsConfigInput {
    providers: std::collections::HashMap<String, ProviderConfigInput>,
}

#[derive(Debug, Deserialize)]
struct ProviderConfigInput {
    api_key: String,
    base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryConfigInput {
    backend: Option<String>,
    embedding_model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChannelsConfigInput {
    telegram: Option<Vec<TelegramConfigInput>>,
}

#[derive(Debug, Deserialize)]
struct TelegramConfigInput {
    account_id: Option<String>,
    bot_token: String,
    allow_from: Option<Vec<String>>,
    group_policy: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HttpRequestConfigInput {
    enabled: Option<bool>,
    search_provider: Option<String>,
    allowed_domains: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct BrowserConfigInput {
    enabled: Option<bool>,
    cdp_host: Option<String>,
    cdp_port: Option<u16>,
    cdp_auto_launch: Option<bool>,
    native_headless: Option<bool>,
    native_chrome_path: Option<String>,
    profile_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ComposioConfigInput {
    enabled: Option<bool>,
    api_key: Option<String>,
    entity_id: Option<String>,
}

async fn save_config(
    State(state): State<AppState>,
    Json(req): Json<SaveConfigRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut config = state.config.write().await;

    // Update fields from request
    if let Some(provider) = req.default_provider {
        config.default_provider = provider;
    }
    if let Some(model) = req.default_model {
        config.default_model = Some(model);
    }

    // Update models
    if let Some(models) = req.models {
        use crate::config::ProviderConfig;
        if let Some(ref mut config_models) = config.models {
            for (name, provider) in models.providers {
                config_models.providers.insert(
                    name,
                    ProviderConfig {
                        api_key: provider.api_key,
                        base_url: provider.base_url,
                        model: None,
                    },
                );
            }
        }
    }

    // Update memory
    if let Some(memory) = req.memory {
        if let Some(backend) = memory.backend {
            config.memory.backend = backend;
        }
        if let Some(embedding_model) = memory.embedding_model {
            config.memory.embedding_model = Some(embedding_model);
        }
    }

    // Update channels
    if let Some(channels) = req.channels
        && let Some(telegram_list) = channels.telegram {
            use crate::config_types::TelegramConfig;
            config.channels.telegram = telegram_list
                .into_iter()
                .map(|t| TelegramConfig {
                    account_id: t.account_id.unwrap_or_else(|| "main".to_string()),
                    bot_token: t.bot_token,
                    allow_from: t.allow_from.unwrap_or_default(),
                    group_policy: t.group_policy.unwrap_or_else(|| "allowlist".to_string()),
                    reply_in_private: true,
                    group_allow_from: vec![],
                    proxy: None,
                })
                .collect();
        }

    // Update http_request
    if let Some(http_req) = req.http_request {
        if let Some(enabled) = http_req.enabled {
            config.http_request.enabled = enabled;
        }
        if let Some(provider) = http_req.search_provider {
            config.http_request.search_provider = provider;
        }
        if let Some(domains) = http_req.allowed_domains {
            config.http_request.allowed_domains = domains;
        }
    }

    // Update browser
    if let Some(ref browser) = req.browser
        && let Some(enabled) = browser.enabled {
            config.browser.enabled = enabled;
        }
    if let Some(browser) = req.browser {
        if let Some(host) = browser.cdp_host {
            config.browser.cdp_host = host;
        }
        if let Some(port) = browser.cdp_port {
            config.browser.cdp_port = port;
        }
        if let Some(auto) = browser.cdp_auto_launch {
            config.browser.cdp_auto_launch = auto;
        }
        if let Some(headless) = browser.native_headless {
            config.browser.native_headless = headless;
        }
        if let Some(path) = browser.native_chrome_path {
            config.browser.native_chrome_path = Some(path);
        }
        if let Some(profile_dir) = browser.profile_dir {
            config.browser.profile_dir = Some(profile_dir);
        }
    }

    // Update composio
    if let Some(composio) = req.composio {
        if let Some(enabled) = composio.enabled {
            config.composio.enabled = enabled;
        }
        if let Some(api_key) = composio.api_key {
            config.composio.api_key = Some(api_key);
        }
        if let Some(entity_id) = composio.entity_id {
            config.composio.entity_id = entity_id;
        }
    }

    // Save to disk
    if let Err(e) = config.save() {
        tracing::error!("Failed to save config: {}", e);
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    Ok(Json(serde_json::json!({ "success": true })))
}

// Chat API (HTTP fallback)
#[derive(Debug, Deserialize)]
struct ChatRequest {
    message: String,
}

#[derive(Debug, Serialize)]
struct ChatResponse {
    response: String,
}

async fn chat_handler(
    State(_state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, StatusCode> {
    // Simple echo response for now - in production this would call the agent
    let response = format!(
        "Received: {}. This is a placeholder response until the agent router is integrated.",
        req.message
    );

    Ok(Json(ChatResponse { response }))
}

// WebSocket handler (placeholder for now)
async fn websocket_handler() -> impl IntoResponse {
    // WebSocket support would be added here in a future iteration
    // For now, return a message indicating it's not yet implemented
    (StatusCode::NOT_IMPLEMENTED, "WebSocket chat coming soon!")
}

#[derive(Debug, Deserialize)]
struct WhatsAppWebhookMessage {
    sender: String,
    chat_id: String,
    content: String,
    platform: String,
}

async fn whatsapp_webhook_handler(
    Json(msg): Json<WhatsAppWebhookMessage>,
) -> impl IntoResponse {
    use crate::bus::{InboundMessage, global_bus};

    let inbound = InboundMessage {
        channel: "whatsapp_native".to_string(),
        sender_id: msg.sender,
        chat_id: msg.chat_id.clone(),
        content: msg.content,
        session_key: format!("whatsapp_native:{}", msg.chat_id),
        media: Vec::new(),
        metadata_json: None,
    };

    if let Some(bus) = global_bus() {
        if let Err(e) = bus.publish_inbound(inbound) {
            tracing::error!("Failed to publish WhatsApp message to bus: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
        StatusCode::OK
    } else {
        tracing::error!("Global bus not initialized");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}
