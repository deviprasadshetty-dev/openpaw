use crate::config_types::{
    BrowserConfig, ChannelsConfig, ComposioConfig, HardwareConfig, HttpRequestConfig,
    McpServerConfig, MemoryConfig,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub default_provider: String,
    pub default_model: Option<String>,
    pub default_temperature: Option<f32>,
    pub models: Option<ModelsConfig>,
    #[serde(default)]
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub channels: ChannelsConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub http_request: HttpRequestConfig,
    #[serde(default)]
    pub browser: BrowserConfig,
    #[serde(default)]
    pub composio: ComposioConfig,
    #[serde(default)]
    pub hardware: HardwareConfig,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub agents: Vec<NamedAgentConfig>,

    #[serde(skip)]
    pub config_path: String,
    #[serde(skip)]
    pub workspace_dir: String,
}

impl Config {
    pub fn default_provider_key(&self) -> Option<String> {
        self.models
            .as_ref()?
            .providers
            .get(&self.default_provider)
            .map(|p| p.api_key.clone())
    }

    pub fn save(&self) -> anyhow::Result<()> {
        if self.config_path.is_empty() {
            return Err(anyhow::anyhow!("No config path set, cannot save"));
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&self.config_path, json)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NamedAgentConfig {
    pub name: String,
    // Add other fields as needed
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelsConfig {
    pub providers: std::collections::HashMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderConfig {
    pub api_key: String,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct GatewayConfig {
    #[serde(default = "default_gateway_port")]
    pub port: u16,
    #[serde(default = "default_gateway_host")]
    pub host: String,
    #[serde(default)]
    pub require_pairing: bool,
    #[serde(default)]
    pub allow_public_bind: Option<bool>,
}

fn default_gateway_port() -> u16 {
    3000
}

fn default_gateway_host() -> String {
    "127.0.0.1".to_string()
}
