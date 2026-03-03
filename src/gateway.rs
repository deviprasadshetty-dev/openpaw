use anyhow::Result;
use axum::{
    Router,
    routing::{get, post},
};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::info;

use crate::config::Config;

pub async fn serve(config: Config) -> Result<()> {
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/pair", post(pair));

    let local_bind = if config.gateway.allow_public_bind.unwrap_or(false) {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };

    let bind_addr = format!("{}:{}", local_bind, config.gateway.port);
    let addr: SocketAddr = bind_addr.parse()?;

    info!("Gateway listening on {}", addr);

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_check() -> &'static str {
    "ok"
}

async fn pair() -> &'static str {
    // Stub implementation for /pair
    "paired"
}
