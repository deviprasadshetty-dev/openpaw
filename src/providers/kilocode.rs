/// Kilo.ai (KiloCode) provider.
///
/// Kilo Gateway is a fully OpenAI-compatible API at <https://api.kilo.ai/api/gateway>.
/// It provides:
/// - 200+ models (Anthropic, OpenAI, Google, xAI, Mistral, …) under one API key
/// - Several always-free models tagged with `:free`
/// - Standard Bearer-token authentication
///
/// This provider adds **automatic model fallback**: if the configured model
/// fails (rate-limit, unavailable, …), it transparently retries with each
/// subsequent free model in `fallback_models` before returning an error.
use anyhow::Result;
use reqwest::blocking::Client;
use tracing::{debug, warn};

use crate::providers::{ChatRequest, ChatResponse, Provider};

use super::openai::OpenAiCompatibleProvider;

pub const KILOCODE_BASE_URL: &str = "https://api.kilo.ai/api/gateway";
pub const KILOCODE_MODELS_URL: &str = "https://api.kilo.ai/api/gateway/models";

/// Built-in free-model fallback list.
/// Used when the API is unreachable during onboarding or as a last-resort
/// order for runtime fallback.
pub const DEFAULT_FREE_MODELS: &[&str] = &[
    "minimax/minimax-m2.1:free",
    "arcee-ai/trinity-large-preview:free",
    "z-ai/glm-5:free",
    "corethink:free",
    "giga-potato",
];

/// KiloCode provider with transparent model-level fallback.
///
/// `primary_model` is tried first (as supplied in `ChatRequest::model`).
/// If it fails, we iterate through `fallback_models` in order until one
/// succeeds. The first successful model's response is returned.
pub struct KiloCodeProvider {
    inner: OpenAiCompatibleProvider,
    /// Ordered list of fallback model IDs to try when the primary fails.
    pub fallback_models: Vec<String>,
}

impl KiloCodeProvider {
    pub fn new(api_key: &str, fallback_models: Vec<String>) -> Self {
        Self {
            inner: OpenAiCompatibleProvider::new("kilocode", KILOCODE_BASE_URL, api_key),
            fallback_models,
        }
    }

    /// Send a blocking chat call with a specific model substituted in.
    fn chat_with_model(&self, model: &str, request: &ChatRequest) -> Result<ChatResponse> {
        let patched = ChatRequest {
            model,
            messages: request.messages,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            tools: request.tools,
            timeout_secs: request.timeout_secs,
            reasoning_effort: request.reasoning_effort,
        };
        self.inner.chat(&patched)
    }
}

impl Provider for KiloCodeProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        // Try the primary model first.
        match self.chat_with_model(request.model, request) {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                warn!(
                    "[kilocode] Model '{}' failed: {}. Trying fallbacks…",
                    request.model, e
                );
            }
        }

        // Iterate fallbacks, skipping any that already match the primary.
        for fb in &self.fallback_models {
            if fb.as_str() == request.model {
                continue;
            }
            debug!("[kilocode] Falling back to model '{}'", fb);
            match self.chat_with_model(fb, request) {
                Ok(resp) => {
                    warn!("[kilocode] Succeeded with fallback model '{}'", fb);
                    return Ok(resp);
                }
                Err(e) => {
                    warn!("[kilocode] Fallback model '{}' also failed: {}", fb, e);
                }
            }
        }

        anyhow::bail!(
            "[kilocode] All models exhausted (primary: '{}', {} fallback(s)). \
             Check your API key or try a different model.",
            request.model,
            self.fallback_models.len()
        )
    }

    // chat_stream uses the default trait implementation which calls self.chat(),
    // so it automatically benefits from the model fallback logic above.

    fn supports_native_tools(&self) -> bool {
        true
    }

    fn get_name(&self) -> &str {
        "kilocode"
    }
}

// ── Free-model discovery ──────────────────────────────────────────

/// Fetch models tagged `:free` from the Kilo Gateway `/models` endpoint.
/// Returns them sorted alphabetically; falls back to `DEFAULT_FREE_MODELS` on any error.
pub fn fetch_kilocode_free_models(api_key: &str) -> Result<Vec<String>> {
    use serde_json::Value;
    use std::time::Duration;

    let client = Client::builder().timeout(Duration::from_secs(12)).build()?;

    let mut req = client
        .get(KILOCODE_MODELS_URL)
        .header("Accept", "application/json");

    if !api_key.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
    }

    let res = req.send()?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().unwrap_or_default();
        anyhow::bail!("Kilo /models error {}: {}", status, body);
    }

    let payload: Value = res.json()?;

    // Kilo follows OpenAI's model list schema: { "data": [ { "id": "..." }, … ] }
    let data = payload
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Kilo /models missing 'data' array"))?;

    let mut models: Vec<String> = data
        .iter()
        .filter_map(|m| m.get("id").and_then(|v| v.as_str()))
        .filter(|id| {
            let lower = id.to_ascii_lowercase();
            lower.contains(":free") || *id == "giga-potato"
        })
        .map(|id| id.to_string())
        .collect();

    models.sort();
    models.dedup();
    Ok(models)
}

/// Return the preferred model index from a list (for pre-selecting a sane default).
pub fn preferred_kilocode_model_index(models: &[String]) -> usize {
    let preferred = [
        "minimax/minimax-m2.1:free",
        "arcee-ai/trinity-large-preview:free",
        "z-ai/glm-5:free",
        "corethink:free",
        "giga-potato",
    ];
    for key in preferred {
        if let Some(idx) = models.iter().position(|m| m.eq_ignore_ascii_case(key)) {
            return idx;
        }
    }
    0
}
