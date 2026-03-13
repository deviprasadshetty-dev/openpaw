/// Provider factory — instantiates the right Provider from config.
///
/// Provider factory — instantiates the right Provider from config.
///
/// Wraps the result in `ReliableProvider` so all providers automatically
/// get retry + backoff without each impl needing to handle it.
use std::sync::Arc;

use tracing::warn;

use crate::providers::anthropic::AnthropicProvider;
use crate::providers::fallback::FallbackProvider;
use crate::providers::kilocode::{
    DEFAULT_FREE_MODELS, KiloCodeProvider, fetch_kilocode_free_models,
};
use crate::providers::ollama::OllamaProvider;
use crate::providers::reliable::ReliableProvider;
use crate::providers::{Provider, gemini::GeminiProvider, openai::OpenAiCompatibleProvider};

/// Config snippet used by the factory. Matches the `providers` map in config.json.
#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    pub api_key: String,
    pub base_url: Option<String>,
    pub model: Option<String>,
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
                let models_res = if tokio::runtime::Handle::try_current().is_ok() {
                    tokio::task::block_in_place(|| fetch_kilocode_free_models(&api_key))
                } else {
                    fetch_kilocode_free_models(&api_key)
                };

                match models_res {
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

        "ollama" => {
            let base_url = cfg.and_then(|c| c.base_url.as_deref());
            Arc::new(OllamaProvider::new(base_url))
        }

        // Covers: openai, openrouter, opencode, or any OpenAI-compatible base_url
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

/// Create all providers configured in config.json and wrap them in a FallbackProvider.
pub fn create_with_fallbacks(
    default_name: &str,
    config: &crate::config::Config,
) -> Arc<dyn Provider> {
    let mut providers = Vec::new();

    // Add the primary requested provider first
    if let Some(cfg) = config
        .models
        .as_ref()
        .and_then(|m| m.providers.get(default_name))
    {
        providers.push(create(
            default_name,
            Some(&ProviderConfig {
                api_key: cfg.api_key.clone(),
                base_url: cfg.base_url.clone(),
                model: cfg.model.clone(),
                max_retries: None,
            }),
        ));
    } else {
        // Even if not configured in the map, try to create it via defaults/env
        providers.push(create(default_name, None));
    }

    // Add all other configured providers as fallbacks
    if let Some(models) = &config.models {
        for (name, cfg) in &models.providers {
            if name == default_name {
                continue;
            }
            providers.push(create(
                name,
                Some(&ProviderConfig {
                    api_key: cfg.api_key.clone(),
                    base_url: cfg.base_url.clone(),
                    model: cfg.model.clone(),
                    max_retries: None,
                }),
            ));
        }
    }

    if providers.len() > 1 {
        Arc::new(FallbackProvider::new(providers))
    } else {
        providers
            .into_iter()
            .next()
            .unwrap_or_else(|| create(default_name, None))
    }
}

fn default_base_url(name: &str) -> &'static str {
    match name {
        "openai" => "https://api.openai.com/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        "opencode" => "https://opencode.ai/zen/v1",
        "kilocode" => "https://api.kilo.ai/api/gateway",
        "ollama" => "http://localhost:11434/v1",
        "lmstudio" => "http://localhost:1234/v1",
        "openai-compatible" => "http://localhost:8080/v1",
        _ => "https://api.openai.com/v1",
    }
}

fn resolve_env_key(name: &str) -> String {
    let env_var = match name {
        "openai" => "OPENAI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "opencode" => "OPENCODE_API_KEY",
        "kilocode" => "KILOCODE_API_KEY",
        "ollama" | "lmstudio" | "openai-compatible" => return String::new(), // no key needed or custom
        _ => "OPENAI_API_KEY",
    };
    std::env::var(env_var).unwrap_or_default()
}
