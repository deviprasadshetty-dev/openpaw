use crate::providers::{ChatRequest, ChatResponse, Provider, TokenUsage, ContentPart};
use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use std::path::PathBuf;

const BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const OAUTH_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 8192;
const CLIENT_ID: &str = "936475272427.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "KWaLJfKpIyrGyVOIF2t66XCO";

/// Authentication method for Gemini.
#[derive(Debug, Clone)]
pub enum GeminiAuth {
    /// Explicit API key from config: sent as `?key=` query parameter.
    ExplicitKey(String),
    /// API key from `GEMINI_API_KEY` env var.
    EnvGeminiKey(String),
    /// API key from `GOOGLE_API_KEY` env var.
    EnvGoogleKey(String),
    /// OAuth access token from `GEMINI_OAUTH_TOKEN` env var.
    EnvOAuthToken(String),
    /// OAuth access token from Gemini CLI: sent as `Authorization: Bearer`.
    OAuthToken(String),
}

impl GeminiAuth {
    pub fn is_api_key(&self) -> bool {
        matches!(
            self,
            GeminiAuth::ExplicitKey(_) | GeminiAuth::EnvGeminiKey(_) | GeminiAuth::EnvGoogleKey(_)
        )
    }

    pub fn credential(&self) -> &str {
        match self {
            GeminiAuth::ExplicitKey(v) => v,
            GeminiAuth::EnvGeminiKey(v) => v,
            GeminiAuth::EnvGoogleKey(v) => v,
            GeminiAuth::EnvOAuthToken(v) => v,
            GeminiAuth::OAuthToken(v) => v,
        }
    }

    pub fn source(&self) -> &str {
        match self {
            GeminiAuth::ExplicitKey(_) => "config",
            GeminiAuth::EnvGeminiKey(_) => "GEMINI_API_KEY env var",
            GeminiAuth::EnvGoogleKey(_) => "GOOGLE_API_KEY env var",
            GeminiAuth::EnvOAuthToken(_) => "GEMINI_OAUTH_TOKEN env var",
            GeminiAuth::OAuthToken(_) => "Gemini CLI OAuth",
        }
    }
}

/// Credentials loaded from the Gemini CLI OAuth token file (~/.gemini/oauth_creds.json).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiCliCredentials {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<i64>,
}

impl GeminiCliCredentials {
    /// Returns true if the token is expired (or within 5 minutes of expiring).
    /// If expires_at is None, the token is treated as never-expiring.
    fn is_expired(&self) -> bool {
        let expiry = match self.expires_at {
            Some(e) => e,
            None => return false,
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let buffer_seconds: i64 = 5 * 60; // 5-minute safety buffer
        now >= (expiry - buffer_seconds)
    }
}

/// OAuth token refresh response from Google.
#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    expires_in: i64,
}

/// Google Gemini provider with support for:
/// - Direct API key (`GEMINI_API_KEY` env var or config)
/// - Gemini CLI OAuth tokens (reuse existing ~/.gemini/ authentication)
pub struct GeminiProvider {
    auth: Option<GeminiAuth>,
    client: Client,
}

impl GeminiProvider {
    pub fn new(api_key: Option<&str>) -> Self {
        let mut auth: Option<GeminiAuth> = None;

        // 1. Explicit key
        if let Some(key) = api_key {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                auth = Some(GeminiAuth::ExplicitKey(trimmed.to_string()));
            }
        }

        // 2. Environment variables (only if no explicit key)
        if auth.is_none() {
            if let Ok(value) = std::env::var("GEMINI_API_KEY") {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    auth = Some(GeminiAuth::EnvGeminiKey(trimmed.to_string()));
                }
            }
        }

        if auth.is_none() {
            if let Ok(value) = std::env::var("GOOGLE_API_KEY") {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    auth = Some(GeminiAuth::EnvGoogleKey(trimmed.to_string()));
                }
            }
        }

        // 2b. GEMINI_OAUTH_TOKEN env var (explicit OAuth token)
        if auth.is_none() {
            if let Ok(value) = std::env::var("GEMINI_OAUTH_TOKEN") {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    auth = Some(GeminiAuth::EnvOAuthToken(trimmed.to_string()));
                }
            }
        }

        // 3. Gemini CLI OAuth token (~/.gemini/oauth_creds.json) as final fallback
        if auth.is_none() {
            if let Some(creds) = Self::try_load_gemini_cli_token() {
                auth = Some(GeminiAuth::OAuthToken(creds.access_token));
            }
        }

        Self {
            auth,
            client: Client::builder().build().unwrap(),
        }
    }

    /// Try to load Gemini CLI OAuth credentials from ~/.gemini/oauth_creds.json.
    /// Returns None on any error (file not found, parse failure).
    /// If token is expired but refresh_token is available, attempts to refresh.
    fn try_load_gemini_cli_token() -> Option<GeminiCliCredentials> {
        let home = dirs::home_dir()?;
        let path = home.join(".gemini").join("oauth_creds.json");

        let content = std::fs::read_to_string(&path).ok()?;
        let mut creds: GeminiCliCredentials = serde_json::from_str(&content).ok()?;

        if creds.is_expired() {
            if let Some(refresh_token) = &creds.refresh_token {
                if let Some(refreshed) = Self::refresh_oauth_token(refresh_token) {
                    // Update credentials with new access token and expiry
                    creds.access_token = refreshed.access_token;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    creds.expires_at = Some(now + refreshed.expires_in);

                    // Persist back to file (best effort)
                    if let Ok(new_json) = serde_json::to_string(&creds) {
                        // Use 0o600 permissions if possible (unix)
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::OpenOptionsExt;
                            let _ = std::fs::OpenOptions::new()
                                .write(true)
                                .create(true)
                                .truncate(true)
                                .mode(0o600)
                                .open(&path)
                                .and_then(|mut f| std::io::Write::write_all(&mut f, new_json.as_bytes()));
                        }
                        #[cfg(not(unix))]
                        {
                            let _ = std::fs::write(&path, new_json);
                        }
                    }
                    
                    return Some(creds);
                }
            }
            // Refresh failed or unavailable
            return None;
        }

        Some(creds)
    }

    /// Refresh an OAuth token using Google's OAuth2 endpoint.
    fn refresh_oauth_token(refresh_token: &str) -> Option<RefreshResponse> {
        // Skip actual network call in tests
        if cfg!(test) {
            return None;
        }

        let client = Client::new();
        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ];

        let res = client.post(OAUTH_TOKEN_URL)
            .form(&params)
            .send()
            .ok()?;

        if res.status().is_success() {
            res.json::<RefreshResponse>().ok()
        } else {
            None
        }
    }

    /// Get authentication source description for diagnostics.
    pub fn auth_source(&self) -> &str {
        match &self.auth {
            Some(auth) => auth.source(),
            None => "none",
        }
    }

    /// Build a Gemini generateContent request body.
    fn build_request_body(
        &self,
        request: &ChatRequest,
    ) -> Result<String> {
        let mut contents = Vec::new();
        let mut system_prompt: Option<String> = None;

        for msg in request.messages {
            if msg.role == "system" {
                system_prompt = Some(msg.content.clone());
                continue;
            }

            // Gemini uses "user" and "model" (not "assistant")
            let role = match msg.role.as_str() {
                "user" | "tool" => "user",
                "assistant" => "model",
                _ => "user", // Default to user for unknown roles
            };

            let mut parts = Vec::new();

            // Handle content_parts for multimodal content
            if let Some(content_parts) = msg.content_parts.as_ref() {
                for part in content_parts {
                    match part {
                        ContentPart::Text(text) => {
                            parts.push(json!({"text": text}));
                        }
                        ContentPart::ImageBase64 { data, media_type } => {
                            parts.push(json!({
                                "inlineData": {
                                    "mimeType": media_type,
                                    "data": data
                                }
                            }));
                        }
                        ContentPart::ImageUrl { url } => {
                            // Gemini doesn't support direct URLs; include as text reference
                            parts.push(json!({"text": format!("[Image: {}]", url)}));
                        }
                    }
                }
            } else {
                parts.push(json!({"text": msg.content}));
            }

            contents.push(json!({
                "role": role,
                "parts": parts
            }));
        }

        let mut body = json!({
            "contents": contents,
            "generationConfig": {
                "temperature": request.temperature,
                "maxOutputTokens": request.max_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
            }
        });

        if let Some(sys) = system_prompt {
            body["systemInstruction"] = json!({
                "parts": [{"text": sys}]
            });
        }

        Ok(body.to_string())
    }

    /// Parse text content from a Gemini generateContent response.
    fn parse_response(&self, body: &str) -> Result<String> {
        let parsed: serde_json::Value = serde_json::from_str(body)
            .context("Failed to parse Gemini response")?;

        // Check for error first
        if let Some(error) = parsed.get("error") {
            let msg = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown Gemini API error");
            let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
            anyhow::bail!("Gemini API error (code {}): {}", code, msg);
        }

        // Check for promptFeedback (blocked content)
        if let Some(feedback) = parsed.get("promptFeedback") {
            if let Some(block_reason) = feedback.get("blockReason") {
                let reason = block_reason.as_str().unwrap_or("unknown");
                anyhow::bail!("Gemini blocked the prompt: {}", reason);
            }
        }

        // Extract text from candidates
        if let Some(candidates) = parsed.get("candidates") {
            if let Some(candidate) = candidates.get(0) {
                // Check for finishReason
                if let Some(finish_reason) = candidate.get("finishReason") {
                    let reason = finish_reason.as_str().unwrap_or("unknown");
                    // STOP and MAX_TOKENS are normal finish reasons
                    if reason != "STOP" && reason != "MAX_TOKENS" {
                        tracing::warn!("Gemini finish reason: {}", reason);
                    }
                }

                if let Some(content) = candidate.get("content") {
                    if let Some(parts) = content.get("parts") {
                        // Concatenate all text parts
                        let mut text_parts = Vec::new();
                        let mut function_calls = Vec::new();
                        for part in parts.as_array().unwrap_or(&vec![]) {
                            if let Some(text) = part.get("text") {
                                if let Some(s) = text.as_str() {
                                    text_parts.push(s.to_string());
                                }
                            }
                            // Handle native function calls (when model outputs them despite not being requested)
                            if let Some(fc) = part.get("functionCall") {
                                if let Some(name) = fc.get("name").and_then(|n| n.as_str()) {
                                    let args = fc.get("args").cloned().unwrap_or(json!({}));
                                    let args_str = args.to_string();
                                    // Convert to XML format that our dispatcher can parse
                                    function_calls.push(format!(
                                        "<tool_call>{{\"name\": \"{}\", \"arguments\": {}}}</tool_call>",
                                        name,
                                        args_str
                                    ));
                                }
                            }
                        }
                        // If we have function calls, append them to the text
                        if !function_calls.is_empty() {
                            text_parts.extend(function_calls);
                        }
                        if !text_parts.is_empty() {
                            return Ok(text_parts.join(""));
                        }
                    }
                }
            }
        }

        // Debug: log the response structure
        tracing::debug!("Gemini response structure: {}", serde_json::to_string_pretty(&parsed).unwrap_or_default());
        anyhow::bail!("No response content from Gemini")
    }
}

impl Provider for GeminiProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let mut auth = self.auth.clone().ok_or_else(|| {
            anyhow::anyhow!("No Gemini credentials configured. Set GEMINI_API_KEY, GOOGLE_API_KEY, GEMINI_OAUTH_TOKEN env var, or configure an API key in settings.")
        })?;

        // Format model name (prepend "models/" if not present)
        let model_name = if request.model.starts_with("models/") {
            request.model.to_string()
        } else {
            format!("models/{}", request.model)
        };

        // Build URL
        let url = if auth.is_api_key() {
            format!(
                "{}/{}:generateContent?key={}",
                BASE_URL,
                model_name,
                auth.credential()
            )
        } else {
            format!("{}/{}:generateContent", BASE_URL, model_name)
        };

        tracing::debug!("Gemini request URL: {}", url);

        let body = self.build_request_body(request)?;
        tracing::debug!("Gemini request body: {}", body);

        let mut req_builder = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(request.timeout_secs))
            .header("Content-Type", "application/json");

        // Add Authorization header for OAuth tokens
        if !auth.is_api_key() {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", auth.credential()));
        }

        let res = req_builder.body(body).send()?;
        
        // Handle 401 Unauthorized for OAuth tokens - try one refresh
        if res.status() == reqwest::StatusCode::UNAUTHORIZED && !auth.is_api_key() {
             // If we're using CLI OAuth, try to refresh explicitly
             if let Some(creds) = Self::try_load_gemini_cli_token() {
                 if let Some(new_token) = &creds.refresh_token {
                      // It seems it was refreshed inside try_load_gemini_cli_token if expired, 
                      // but here we got 401 even if we thought it was valid.
                      // Let's force a refresh logic if we could, but try_load logic handles expiry check.
                      // If we are here, it means the token was considered valid by time but rejected by API.
                      // Or it means we loaded a stale token initially.
                      // Let's rely on re-loading the token.
                      auth = GeminiAuth::OAuthToken(creds.access_token);
                      
                      // Retry request
                      let mut retry_builder = self
                        .client
                        .post(&url)
                        .timeout(Duration::from_secs(request.timeout_secs))
                        .header("Content-Type", "application/json")
                        .header("Authorization", format!("Bearer {}", auth.credential()));
                        
                      let body_retry = self.build_request_body(request)?;
                      let res_retry = retry_builder.body(body_retry).send()?;
                      
                      if !res_retry.status().is_success() {
                           let status = res_retry.status();
                           let text = res_retry.text().unwrap_or_default();
                           anyhow::bail!("Gemini API error after refresh {}: {}", status, text);
                      }
                      
                      let resp_text = res_retry.text()?;
                      let content = self.parse_response(&resp_text)?;
                      
                      // Calculate usage
                      let prompt_tokens: u32 = request.messages.iter().map(|m| m.content.len() as u32 / 4).sum();
                      let completion_tokens = (content.len() as u32 + 3) / 4;
                      
                      return Ok(ChatResponse {
                        content: Some(content),
                        tool_calls: Vec::new(),
                        usage: TokenUsage {
                            prompt_tokens,
                            completion_tokens,
                            total_tokens: prompt_tokens + completion_tokens,
                        },
                        model: request.model.to_string(),
                        reasoning_content: None,
                    });
                 }
             }
        }

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().unwrap_or_default();
            anyhow::bail!("Gemini API error {}: {}", status, text);
        }

        let resp_text = res.text()?;
        tracing::debug!("Gemini response: {}", resp_text);
        let content = self.parse_response(&resp_text)?;

        // Estimate token usage (Gemini doesn't always return usage)
        let prompt_tokens: u32 = request
            .messages
            .iter()
            .map(|m| m.content.len() as u32 / 4)
            .sum();
        let completion_tokens = (content.len() as u32 + 3) / 4;

        Ok(ChatResponse {
            content: Some(content),
            tool_calls: Vec::new(),
            usage: TokenUsage {
                prompt_tokens,
                completion_tokens,
                total_tokens: prompt_tokens + completion_tokens,
            },
            model: request.model.to_string(),
            reasoning_content: None,
        })
    }

    fn supports_native_tools(&self) -> bool {
        // Gemini supports tools but we'll implement basic support first
        false
    }

    fn get_name(&self) -> &str {
        "gemini"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_auth_is_api_key() {
        let key = GeminiAuth::ExplicitKey("key".to_string());
        assert!(key.is_api_key());

        let oauth = GeminiAuth::OAuthToken("ya29.token".to_string());
        assert!(!oauth.is_api_key());
    }

    #[test]
    fn test_gemini_auth_source() {
        assert_eq!(
            GeminiAuth::ExplicitKey("k".to_string()).source(),
            "config"
        );
        assert_eq!(
            GeminiAuth::EnvGeminiKey("k".to_string()).source(),
            "GEMINI_API_KEY env var"
        );
        assert_eq!(
            GeminiAuth::EnvGoogleKey("k".to_string()).source(),
            "GOOGLE_API_KEY env var"
        );
        assert_eq!(
            GeminiAuth::OAuthToken("t".to_string()).source(),
            "Gemini CLI OAuth"
        );
    }

    #[test]
    fn test_gemini_credentials_expired() {
        let future = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600; // 1 hour from now

        let creds = GeminiCliCredentials {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Some(future),
        };
        assert!(!creds.is_expired());

        let past = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - 3600; // 1 hour ago

        let creds_expired = GeminiCliCredentials {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Some(past),
        };
        assert!(creds_expired.is_expired());
    }
}
