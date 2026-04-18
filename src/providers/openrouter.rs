/// OpenRouter provider.
///
/// OpenRouter is a unified AI gateway that gives access to 200+ models from all
/// major providers (Anthropic, OpenAI, Google, Mistral, Meta, …) via a single key
/// and an OpenAI-compatible API at <https://openrouter.ai/api/v1>.
///
/// # Free models
/// OpenRouter exposes many models at zero cost. To find them, query `GET /models`
/// and filter by `pricing.prompt == "0"` AND `pricing.completion == "0"`.
///
/// Because free models can have tiny, unusable context windows, we enforce a
/// minimum of `MIN_CONTEXT_TOKENS` (8 192). This keeps only models with enough
/// headroom for a system prompt + memory + tool schemas, filtering out 4 K-class
/// models that aren't useful for an agent.
use anyhow::Result;
use reqwest::blocking::Client;

use crate::providers::{ChatRequest, ChatResponse, Provider};

use super::openai::OpenAiCompatibleProvider;

pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";

/// Minimum context window to consider a free model "usable" for an AI agent.
///
/// 8 192 tokens is a practical floor: system prompt (~2 K) + memory (~2 K) +
/// tool schemas (~1–2 K) + a few conversation turns still fits comfortably.
/// Most good free models today offer 32 K–512 K, so this is a liberal filter.
pub const MIN_CONTEXT_TOKENS: u64 = 8_192;

pub struct OpenRouterProvider {
    inner: OpenAiCompatibleProvider,
}

impl OpenRouterProvider {
    pub fn new(api_key: &str) -> Self {
        Self {
            inner: OpenAiCompatibleProvider::new("openrouter", OPENROUTER_BASE_URL, api_key),
        }
    }
}

impl Provider for OpenRouterProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        // OpenRouter uses the exact same Chat Completions API format as OpenAI.
        self.inner.chat(request)
    }

    fn supports_native_tools(&self) -> bool {
        self.inner.supports_native_tools()
    }

    fn get_name(&self) -> &str {
        self.inner.get_name()
    }
}

// ── Free-model discovery ──────────────────────────────────────────

/// A free model available on OpenRouter, including its context window.
#[derive(Debug, Clone)]
pub struct OpenRouterFreeModel {
    pub id: String,
    pub name: String,
    pub context_length: u64,
}

/// Fetch models that are both free (prompt + completion cost == 0) and have at
/// least `MIN_CONTEXT_TOKENS` context, from the OpenRouter `/models` endpoint.
///
/// Models are sorted by context window descending (largest first), then by id
/// alphabetically as a tiebreaker so the list is stable and the best models
/// float to the top.
pub fn fetch_openrouter_free_models(api_key: &str) -> Result<Vec<OpenRouterFreeModel>> {
    use serde_json::Value;
    use std::time::Duration;

    let client = Client::builder().timeout(Duration::from_secs(15)).build()?;

    let mut req = client
        .get(OPENROUTER_MODELS_URL)
        .header("Accept", "application/json");

    if !api_key.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
    }

    let res = req.send()?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().unwrap_or_default();
        anyhow::bail!("OpenRouter /models error {}: {}", status, body);
    }

    let payload: Value = res.json()?;
    let data = payload
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("OpenRouter /models missing 'data' array"))?;

    let mut models: Vec<OpenRouterFreeModel> = data
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            let name = m
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();

            // context_length field (number of tokens)
            let context_length = m
                .get("context_length")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            // pricing object: both prompt and completion must be "0" (or 0)
            let pricing = m.get("pricing")?;
            let prompt_cost = pricing.get("prompt").and_then(|v| {
                // Can be a string "0" or a number 0
                v.as_str()
                    .map(|s| s.parse::<f64>().unwrap_or(1.0))
                    .or_else(|| v.as_f64())
            })?;
            let completion_cost = pricing.get("completion").and_then(|v| {
                v.as_str()
                    .map(|s| s.parse::<f64>().unwrap_or(1.0))
                    .or_else(|| v.as_f64())
            })?;

            // Must be truly free and meet the minimum context requirement
            if prompt_cost != 0.0 || completion_cost != 0.0 {
                return None;
            }
            if context_length < MIN_CONTEXT_TOKENS {
                return None;
            }

            Some(OpenRouterFreeModel {
                id,
                name,
                context_length,
            })
        })
        .collect();

    // Sort: largest context first, then alphabetically by id
    models.sort_by(|a, b| {
        b.context_length
            .cmp(&a.context_length)
            .then(a.id.cmp(&b.id))
    });
    models.dedup_by(|a, b| a.id == b.id);

    Ok(models)
}

/// Return a display label for a model (id + context in K).
pub fn format_openrouter_model(m: &OpenRouterFreeModel) -> String {
    let ctx_k = m.context_length / 1_000;
    format!("{:<52} {}K ctx", m.id, ctx_k)
}

/// Pick a preferred default from the fetched free-model list.
/// Prefers well-known capable models; falls back to index 0.
pub fn preferred_openrouter_model_index(models: &[OpenRouterFreeModel]) -> usize {
    let preferred = [
        // OpenRouter's own free router — picks best model automatically
        "openrouter/optimus-alpha:free",
        // Strong recent free models
        "google/gemma-3-27b-it:free",
        "meta-llama/llama-4-maverick:free",
        "meta-llama/llama-4-scout:free",
        "deepseek/deepseek-chat-v3-0324:free",
        "qwen/qwen3-235b-a22b:free",
        "mistralai/mistral-small-3.2-24b-instruct:free",
        "google/gemma-3-12b-it:free",
    ];

    for key in preferred {
        if let Some(idx) = models.iter().position(|m| m.id.eq_ignore_ascii_case(key)) {
            return idx;
        }
    }
    0
}
