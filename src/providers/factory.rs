/// Provider factory — instantiates the right Provider from config.
///
/// Provider factory — instantiates the right Provider from config.
///
/// Wraps the result in `ReliableProvider` so all providers automatically
/// get retry + backoff without each impl needing to handle it.
use std::sync::Arc;

use tracing::warn;

use crate::providers::anthropic::AnthropicProvider;
use crate::providers::kilocode::{
    DEFAULT_FREE_MODELS, KiloCodeProvider, fetch_kilocode_free_models,
};
use crate::providers::reliable::ReliableProvider;
use crate::providers::{Provider, gemini::GeminiProvider, openai::OpenAiCompatibleProvider};

/// Config snippet used by the factory. Matches the `providers` map in config.json.
#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    pub api_key: String,
    pub base_url: Option<String>,
    pub max_retries: Option<u32>,
}

/// Create a provider by name with optional config.
/// Always wrapped in `ReliableProvider`.
pub fn create(name: &str, cfg: Option<&ProviderConfig>) -> Arc<dyn Provider> {
    let max_retries = cfg.and_then(|c| c.max_retries).unwrap_or(3);

    let inner: Arc<dyn Provider> = match name {
        "anthropic" => {
            let key = cfg.map(|c| c.api_key.clone()).unwrap_or_default();
            let resolved =
                AnthropicProvider::resolve_key(if key.is_empty() { None } else { Some(&key) });
            if resolved.is_empty() {
                warn!("ANTHROPIC_API_KEY not set — Anthropic provider may fail");
            }
            Arc::new(AnthropicProvider::new(resolved))
        }

        "gemini" => {
            let key = cfg.map(|c| c.api_key.clone());
            Arc::new(GeminiProvider::new(key.as_deref()))
        }

        "kilocode" => {
            let api_key = cfg
                .map(|c| c.api_key.clone())
                .unwrap_or_else(|| resolve_env_key("kilocode"));

            // Try to fetch live free models from the gateway; fall back to built-in list.
            let fallback_models: Vec<String> = if api_key.is_empty() {
                DEFAULT_FREE_MODELS.iter().map(|s| s.to_string()).collect()
            } else {
                match fetch_kilocode_free_models(&api_key) {
                    Ok(models) if !models.is_empty() => {
                        tracing::debug!(
                            "[kilocode] Loaded {} free fallback model(s)",
                            models.len()
                        );
                        models
                    }
                    Ok(_) | Err(_) => {
                        tracing::warn!(
                            "[kilocode] Could not fetch free models; using built-in fallback list"
                        );
                        DEFAULT_FREE_MODELS.iter().map(|s| s.to_string()).collect()
                    }
                }
            };

            Arc::new(KiloCodeProvider::new(&api_key, fallback_models))
        }

        // Covers: openai, openrouter, opencode, ollama, or any OpenAI-compatible base_url
        _ => {
            let base_url = cfg
                .and_then(|c| c.base_url.clone())
                .unwrap_or_else(|| default_base_url(name).to_string());

            let api_key = cfg
                .map(|c| c.api_key.clone())
                .unwrap_or_else(|| resolve_env_key(name));

            Arc::new(OpenAiCompatibleProvider::new(name, &base_url, &api_key))
        }
    };

    Arc::new(ReliableProvider::new(inner).with_retries(max_retries))
}

fn default_base_url(name: &str) -> &'static str {
    match name {
        "openai" => "https://api.openai.com/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        "opencode" => "https://opencode.ai/zen/v1",
        "kilocode" => "https://api.kilo.ai/api/gateway",
        "ollama" => "http://localhost:11434/v1",
        _ => "https://api.openai.com/v1",
    }
}

fn resolve_env_key(name: &str) -> String {
    let env_var = match name {
        "openai" => "OPENAI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "opencode" => "OPENCODE_API_KEY",
        "kilocode" => "KILOCODE_API_KEY",
        "ollama" => return String::new(), // no key needed
        _ => "OPENAI_API_KEY",
    };
    std::env::var(env_var).unwrap_or_default()
}
