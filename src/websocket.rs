use anyhow::{anyhow, Result};

pub struct WsClient;

impl WsClient {
    pub fn connect(_url: &str) -> Result<Self> {
        Err(anyhow!("WebSocket support not enabled (requires tungstenite crate)"))
    }

    pub fn send(&self, _msg: &str) -> Result<()> {
        Err(anyhow!("Not connected"))
    }

    pub fn receive(&self) -> Result<String> {
        Err(anyhow!("Not connected"))
    }
}
