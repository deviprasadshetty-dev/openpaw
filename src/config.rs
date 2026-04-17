use crate::config_types::{
    AgentBinding, BrowserConfig, ChannelsConfig, ComposioConfig, HardwareConfig, HttpRequestConfig,
    McpServerConfig, MemoryConfig, NamedAgentConfig, OpencodeCliConfig, PushoverConfig,
    ReliabilityConfig, SchedulerConfig, SessionConfig,
};
use crate::secrets::SecretStore;
use serde::{Deserialize, Serialize};
use std::path::Path;

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
    pub opencode_cli: OpencodeCliConfig,
    #[serde(default)]
    pub browser: BrowserConfig,
    #[serde(default)]
    pub composio: ComposioConfig,
    #[serde(default)]
    pub hardware: HardwareConfig,
    #[serde(default)]
    pub pushover: PushoverConfig,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub agents: Vec<NamedAgentConfig>,
    #[serde(default)]
    pub reliability: ReliabilityConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub bindings: Vec<AgentBinding>,

    #[serde(skip)]
    pub config_path: String,
    #[serde(skip)]
    pub workspace_dir: String,
    #[serde(skip)]
    pub secret_store: Option<SecretStore>,
}

impl Config {
    /// Initialize the secret store for this config.
    pub fn init_secret_store(&mut self, enabled: bool) -> anyhow::Result<()> {
        let config_dir = Path::new(&self.config_path)
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Config path has no parent directory"))?;

        self.secret_store = Some(SecretStore::new(config_dir, enabled));
        Ok(())
    }

    /// Encrypt all secrets in the config before saving.
    pub fn encrypt_secrets(&mut self) -> anyhow::Result<()> {
        let Some(ref store) = self.secret_store else {
            return Ok(()); // No secret store, skip encryption
        };

        // Encrypt provider API keys
        if let Some(ref mut models) = self.models {
            for (_, provider) in models.providers.iter_mut() {
                if !provider.api_key.is_empty() && !SecretStore::is_encrypted(&provider.api_key) {
                    provider.api_key = store.encrypt(&provider.api_key)?;
                }
            }
        }

        // Encrypt Telegram bot tokens (telegram is a Vec)
        for account in self.channels.telegram.iter_mut() {
            if !account.bot_token.is_empty() && !SecretStore::is_encrypted(&account.bot_token) {
                account.bot_token = store.encrypt(&account.bot_token)?;
            }
        }

        // Encrypt Composio API key
        if self.composio.enabled {
            if let Some(ref mut api_key) = self.composio.api_key {
                if !api_key.is_empty() && !SecretStore::is_encrypted(api_key) {
                    *api_key = store.encrypt(api_key)?;
                }
            }
        }

        Ok(())
    }

    /// Decrypt all secrets in the config after loading.
    pub fn decrypt_secrets(&mut self) -> anyhow::Result<()> {
        let Some(ref store) = self.secret_store else {
            return Ok(()); // No secret store, skip decryption
        };

        // Decrypt provider API keys
        if let Some(ref mut models) = self.models {
            for (_, provider) in models.providers.iter_mut() {
                if SecretStore::is_encrypted(&provider.api_key) {
                    provider.api_key = store.decrypt(&provider.api_key)?;
                }
            }
        }

        // Decrypt Telegram bot tokens (telegram is a Vec)
        for account in self.channels.telegram.iter_mut() {
            if SecretStore::is_encrypted(&account.bot_token) {
                account.bot_token = store.decrypt(&account.bot_token)?;
            }
        }

        // Decrypt Composio API key
        if self.composio.enabled {
            if let Some(ref mut api_key) = self.composio.api_key {
                if SecretStore::is_encrypted(api_key) {
                    *api_key = store.decrypt(api_key)?;
                }
            }
        }

        Ok(())
    }

    pub fn default_provider_key(&self) -> Option<String> {
        self.models
            .as_ref()?
            .providers
            .get(&self.default_provider)
            .map(|p| p.api_key.clone())
    }

    pub fn get_model_for_provider(&self, name: &str) -> Option<String> {
        if let Some(models) = &self.models
            && let Some(p_cfg) = models.providers.get(name)
            && let Some(m) = &p_cfg.model
        {
            return Some(m.clone());
        }
        self.default_model.clone()
    }

    pub fn save(&mut self) -> anyhow::Result<()> {
        if self.config_path.is_empty() {
            return Err(anyhow::anyhow!("No config path set, cannot save"));
        }

        // Encrypt secrets before saving
        self.encrypt_secrets()?;

        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&self.config_path, json)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelsConfig {
    pub providers: std::collections::HashMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderConfig {
    pub api_key: String,
    pub base_url: Option<String>,
    pub model: Option<String>,
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
