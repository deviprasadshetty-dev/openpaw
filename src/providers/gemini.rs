use crate::providers::{
    ChatRequest, ChatResponse, ContentPart, Provider, StreamCallback, TokenUsage,
};
use anyhow::{Context, Result};
use regex::Regex;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

const BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const CODE_ASSIST_BASE_URLS: &[&str] = &[
    "https://cloudcode-pa.googleapis.com/v1internal",
    "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal",
    "https://autopush-cloudcode-pa.sandbox.googleapis.com/v1internal",
];
const OAUTH_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 8192;
const CLI_OAUTH_SENTINEL: &str = "cli_oauth";
const OAUTH_CLIENT_ID_KEYS: &[&str] = &[
    "OPENPAW_GEMINI_OAUTH_CLIENT_ID",
    "OPENCLAW_GEMINI_OAUTH_CLIENT_ID",
    "GEMINI_CLI_OAUTH_CLIENT_ID",
];
const OAUTH_CLIENT_SECRET_KEYS: &[&str] = &[
    "OPENPAW_GEMINI_OAUTH_CLIENT_SECRET",
    "OPENCLAW_GEMINI_OAUTH_CLIENT_SECRET",
    "GEMINI_CLI_OAUTH_CLIENT_SECRET",
];
const GEMINI_CLI_OAUTH_SEARCH_DEPTH: usize = 10;

#[derive(Debug, Clone)]
struct OAuthClientCandidate {
    client_id: String,
    client_secret: String,
}

#[derive(Debug, Clone)]
struct CodeAssistContext {
    base_url: &'static str,
    project: String,
}

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
    #[serde(alias = "expiry_date")]
    expires_at: Option<i64>,
}

impl GeminiCliCredentials {
    /// Returns true if the token is expired (or within 5 minutes of expiring).
    /// If expires_at is None, the token is treated as never-expiring.
    fn is_expired(&self) -> bool {
        let mut expiry = match self.expires_at {
            Some(e) => e,
            None => return false,
        };

        // Handle millisecond timestamps (Google's format) vs seconds
        if expiry > 2000000000 {
            expiry /= 1000;
        }

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
    fn is_cli_oauth_sentinel(value: &str) -> bool {
        value.eq_ignore_ascii_case(CLI_OAUTH_SENTINEL)
    }

    pub fn new(api_key: Option<&str>) -> Self {
        let mut auth: Option<GeminiAuth> = None;
        let mut force_cli_oauth = false;

        // 1. Explicit key
        if let Some(key) = api_key {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                if Self::is_cli_oauth_sentinel(trimmed) {
                    force_cli_oauth = true;
                    tracing::info!("Gemini provider configured to use CLI OAuth mode");
                } else {
                    auth = Some(GeminiAuth::ExplicitKey(trimmed.to_string()));
                }
            }
        }

        // 2. Environment API keys (only if no explicit key and not forced CLI OAuth mode)
        if auth.is_none() && !force_cli_oauth
            && let Ok(value) = std::env::var("GEMINI_API_KEY") {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    auth = Some(GeminiAuth::EnvGeminiKey(trimmed.to_string()));
                }
            }

        if auth.is_none() && !force_cli_oauth
            && let Ok(value) = std::env::var("GOOGLE_API_KEY") {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    auth = Some(GeminiAuth::EnvGoogleKey(trimmed.to_string()));
                }
            }

        // 2b. GEMINI_OAUTH_TOKEN env var (explicit OAuth token)
        if auth.is_none()
            && let Ok(value) = std::env::var("GEMINI_OAUTH_TOKEN") {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    auth = Some(GeminiAuth::EnvOAuthToken(trimmed.to_string()));
                }
            }

        // 3. Gemini CLI OAuth token (~/.gemini/oauth_creds.json) as final fallback
        if auth.is_none() {
            match Self::try_load_gemini_cli_token() {
                Some(creds) => {
                    tracing::info!("Loaded Gemini credentials from CLI OAuth");
                    auth = Some(GeminiAuth::OAuthToken(creds.access_token));
                }
                None => {
                    tracing::debug!("Gemini CLI OAuth token not found or invalid");
                }
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
        let trimmed = content.trim();

        // Very basic attempt to find the JSON object if there's trailing junk
        let json_start = trimmed.find('{')?;
        let json_end = trimmed.rfind('}')?;
        let json_str = &trimmed[json_start..=json_end];

        let mut creds: GeminiCliCredentials = serde_json::from_str(json_str).ok()?;

        if creds.is_expired() {
            if let Some(refresh_token) = &creds.refresh_token
                && let Some(refreshed) = Self::refresh_oauth_token(refresh_token) {
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
                                .and_then(|mut f| {
                                    std::io::Write::write_all(&mut f, new_json.as_bytes())
                                });
                        }
                        #[cfg(not(unix))]
                        {
                            let _ = std::fs::write(&path, new_json);
                        }
                    }

                    return Some(creds);
                }
            // Refresh failed or unavailable
            return None;
        }

        Some(creds)
    }

    fn env_value(keys: &[&str]) -> Option<String> {
        for key in keys {
            if let Ok(value) = std::env::var(key) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }

    fn resolve_oauth_clients() -> Vec<OAuthClientCandidate> {
        let mut out = Vec::new();

        if let (Some(client_id), Some(client_secret)) = (
            Self::env_value(OAUTH_CLIENT_ID_KEYS),
            Self::env_value(OAUTH_CLIENT_SECRET_KEYS),
        ) {
            out.push(OAuthClientCandidate {
                client_id,
                client_secret,
            });
        }

        if let Some((client_id, client_secret)) = Self::extract_gemini_cli_credentials()
            && !out
                .iter()
                .any(|c| c.client_id == client_id && c.client_secret == client_secret)
            {
                out.push(OAuthClientCandidate {
                    client_id,
                    client_secret,
                });
            }

        out
    }

    fn parse_oauth_client_from_js(content: &str) -> Option<(String, String)> {
        let id_re = Regex::new(r"(\d+-[a-z0-9]+\.apps\.googleusercontent\.com)").ok()?;
        let secret_re = Regex::new(r"(GOCSPX-[A-Za-z0-9_-]+)").ok()?;

        let client_id = id_re
            .captures(content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())?;
        let client_secret = secret_re
            .captures(content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())?;
        Some((client_id, client_secret))
    }

    fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::<String>::new();
        for path in paths {
            let key = if cfg!(windows) {
                path.to_string_lossy().replace('\\', "/").to_lowercase()
            } else {
                path.to_string_lossy().to_string()
            };
            if seen.insert(key) {
                out.push(path);
            }
        }
        out
    }

    fn find_file_recursive(root: &Path, filename: &str, max_depth: usize) -> Option<PathBuf> {
        let mut queue = VecDeque::new();
        queue.push_back((root.to_path_buf(), 0usize));

        while let Some((dir, depth)) = queue.pop_front() {
            if depth > max_depth {
                continue;
            }
            let entries = match std::fs::read_dir(&dir) {
                Ok(v) => v,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let file_type = match entry.file_type() {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };
                if file_type.is_file() {
                    if path.file_name().and_then(|v| v.to_str()) == Some(filename) {
                        return Some(path);
                    }
                    continue;
                }
                if file_type.is_dir() {
                    if let Some(name) = path.file_name().and_then(|v| v.to_str())
                        && name.starts_with('.') {
                            continue;
                        }
                    queue.push_back((path, depth + 1));
                }
            }
        }
        None
    }

    fn extract_gemini_cli_credentials() -> Option<(String, String)> {
        let gemini_bin = which::which("gemini").ok()?;
        let resolved = std::fs::canonicalize(&gemini_bin).unwrap_or(gemini_bin.clone());
        let bin_dir = gemini_bin.parent()?;

        let mut cli_dirs = Vec::new();
        if let Some(p) = resolved.parent().and_then(|p| p.parent()) {
            cli_dirs.push(p.to_path_buf());
        }
        if let Some(p) = resolved.parent() {
            cli_dirs.push(p.join("node_modules").join("@google").join("gemini-cli"));
        }
        cli_dirs.push(
            bin_dir
                .join("node_modules")
                .join("@google")
                .join("gemini-cli"),
        );
        if let Some(parent) = bin_dir.parent() {
            cli_dirs.push(
                parent
                    .join("node_modules")
                    .join("@google")
                    .join("gemini-cli"),
            );
            cli_dirs.push(
                parent
                    .join("lib")
                    .join("node_modules")
                    .join("@google")
                    .join("gemini-cli"),
            );
        }

        // Windows global npm path fallback
        if cfg!(windows) {
            if let Ok(appdata) = std::env::var("APPDATA") {
                cli_dirs.push(
                    PathBuf::from(appdata)
                        .join("npm")
                        .join("node_modules")
                        .join("@google")
                        .join("gemini-cli"),
                );
            }
        }

        for cli_dir in Self::dedupe_paths(cli_dirs) {
            let known_paths = [
                cli_dir
                    .join("node_modules")
                    .join("@google")
                    .join("gemini-cli-core")
                    .join("dist")
                    .join("src")
                    .join("code_assist")
                    .join("oauth2.js"),
                cli_dir
                    .join("node_modules")
                    .join("@google")
                    .join("gemini-cli-core")
                    .join("dist")
                    .join("code_assist")
                    .join("oauth2.js"),
            ];
            for path in known_paths {
                if !path.exists() {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&path)
                    && let Some(parsed) = Self::parse_oauth_client_from_js(&content) {
                        return Some(parsed);
                    }
            }

            if let Some(found) =
                Self::find_file_recursive(&cli_dir, "oauth2.js", GEMINI_CLI_OAUTH_SEARCH_DEPTH)
                && let Ok(content) = std::fs::read_to_string(found)
                    && let Some(parsed) = Self::parse_oauth_client_from_js(&content) {
                        return Some(parsed);
                    }
        }
        None
    }

    /// Refresh an OAuth token using Google's OAuth2 endpoint.
    fn refresh_oauth_token(refresh_token: &str) -> Option<RefreshResponse> {
        // Skip actual network call in tests
        if cfg!(test) {
            return None;
        }

        let oauth_clients = Self::resolve_oauth_clients();
        if oauth_clients.is_empty() {
            tracing::debug!(
                "Gemini OAuth refresh skipped: no OAuth client credentials found (set GEMINI_CLI_OAUTH_CLIENT_ID/SECRET or install gemini CLI)"
            );
            return None;
        }

        let client = Client::new();
        for (idx, candidate) in oauth_clients.iter().enumerate() {
            let params = [
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", candidate.client_id.as_str()),
                ("client_secret", candidate.client_secret.as_str()),
            ];

            match client.post(OAUTH_TOKEN_URL).form(&params).send() {
                Ok(res) if res.status().is_success() => {
                    if let Ok(parsed) = res.json::<RefreshResponse>() {
                        return Some(parsed);
                    }
                    tracing::debug!(
                        "Gemini OAuth refresh succeeded but response parse failed for candidate {}",
                        idx
                    );
                }
                Ok(res) => {
                    tracing::debug!(
                        "Gemini OAuth refresh rejected for candidate {}: {}",
                        idx,
                        res.status()
                    );
                }
                Err(err) => {
                    tracing::debug!(
                        "Gemini OAuth refresh request failed for candidate {}: {}",
                        idx,
                        err
                    );
                }
            }
        }

        None
    }

    /// Get authentication source description for diagnostics.
    pub fn auth_source(&self) -> &str {
        match &self.auth {
            Some(auth) => auth.source(),
            None => "none",
        }
    }

    fn uses_code_assist_endpoint(auth: &GeminiAuth) -> bool {
        matches!(
            auth,
            GeminiAuth::OAuthToken(_) | GeminiAuth::EnvOAuthToken(_)
        )
    }

    fn env_code_assist_project() -> Option<String> {
        let project = std::env::var("GOOGLE_CLOUD_PROJECT")
            .or_else(|_| std::env::var("GOOGLE_CLOUD_PROJECT_ID"))
            .ok()?;
        let trimmed = project.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    fn code_assist_platform() -> &'static str {
        if cfg!(target_os = "windows") {
            "WINDOWS"
        } else if cfg!(target_os = "macos") {
            "MACOS"
        } else {
            "PLATFORM_UNSPECIFIED"
        }
    }

    fn parse_code_assist_project(parsed: &serde_json::Value) -> Option<String> {
        if let Some(project) = parsed
            .get("cloudaicompanionProject")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            return Some(project.to_string());
        }
        if let Some(project) = parsed
            .get("cloudaicompanionProject")
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            return Some(project.to_string());
        }
        None
    }

    fn resolve_code_assist_context(
        &self,
        oauth_token: &str,
        timeout_secs: u64,
    ) -> Result<CodeAssistContext> {
        let env_project = Self::env_code_assist_project();
        let metadata = json!({
            "ideType": "ANTIGRAVITY",
            "platform": Self::code_assist_platform(),
            "pluginType": "GEMINI"
        });
        let mut payload = json!({
            "metadata": metadata
        });
        if let Some(project) = env_project.as_ref() {
            payload["cloudaicompanionProject"] = json!(project);
            payload["metadata"]["duetProject"] = json!(project);
        }

        let metadata_header = serde_json::to_string(&payload["metadata"]).unwrap_or_default();
        let mut last_error: Option<anyhow::Error> = None;

        for base_url in CODE_ASSIST_BASE_URLS {
            let url = format!("{}:loadCodeAssist", base_url);
            let res = match self
                .client
                .post(&url)
                .timeout(Duration::from_secs(timeout_secs))
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {}", oauth_token))
                .header("User-Agent", "google-api-nodejs-client/9.15.1")
                .header("X-Goog-Api-Client", "gl-rust/openpaw")
                .header("Client-Metadata", metadata_header.clone())
                .body(payload.to_string())
                .send()
            {
                Ok(res) => res,
                Err(err) => {
                    last_error = Some(err.into());
                    continue;
                }
            };

            if !res.status().is_success() {
                let status = res.status();
                let text = res.text().unwrap_or_default();
                last_error = Some(anyhow::anyhow!(
                    "Gemini Code Assist loadCodeAssist error {}: {}",
                    status,
                    text
                ));
                continue;
            }

            let body = match res.text() {
                Ok(v) => v,
                Err(err) => {
                    last_error = Some(err.into());
                    continue;
                }
            };
            let parsed: serde_json::Value = match serde_json::from_str(&body)
                .context("Failed to parse Code Assist loadCodeAssist response")
            {
                Ok(v) => v,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };

            if let Some(project) = Self::parse_code_assist_project(&parsed) {
                return Ok(CodeAssistContext { base_url, project });
            }
            if let Some(project) = env_project.clone() {
                return Ok(CodeAssistContext { base_url, project });
            }
            last_error = Some(anyhow::anyhow!(
                "Code Assist endpoint returned no project ID"
            ));
        }

        if let Some(project) = env_project {
            return Ok(CodeAssistContext {
                base_url: CODE_ASSIST_BASE_URLS[0],
                project,
            });
        }

        if let Some(err) = last_error {
            return Err(err);
        }
        anyhow::bail!(
            "Gemini CLI OAuth is active but no Code Assist project was resolved. Set GOOGLE_CLOUD_PROJECT and retry."
        )
    }

    fn generate_user_prompt_id() -> String {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        format!("openpaw-{}", millis)
    }

    fn build_code_assist_request_body(
        &self,
        request: &ChatRequest,
        project: &str,
    ) -> Result<String> {
        let request_body_str = self.build_request_body(request)?;
        let request_body: serde_json::Value = serde_json::from_str(&request_body_str)
            .context("Failed to parse Gemini request body for Code Assist")?;

        let model = request
            .model
            .strip_prefix("models/")
            .unwrap_or(request.model);

        Ok(json!({
            "model": model,
            "project": project,
            "user_prompt_id": Self::generate_user_prompt_id(),
            "request": request_body
        })
        .to_string())
    }

    fn build_request_target(
        &self,
        request: &ChatRequest,
        auth: &GeminiAuth,
        streaming: bool,
    ) -> Result<(String, String)> {
        let model_name = if request.model.starts_with("models/") {
            request.model.to_string()
        } else {
            format!("models/{}", request.model)
        };

        let action = if streaming {
            "streamGenerateContent?alt=sse"
        } else {
            "generateContent"
        };

        if Self::uses_code_assist_endpoint(auth) {
            match self.resolve_code_assist_context(auth.credential(), request.timeout_secs) {
                Ok(ctx) => {
                    return Ok((
                        format!("{}:{}", ctx.base_url, action),
                        self.build_code_assist_request_body(request, &ctx.project)?,
                    ));
                }
                Err(err) => {
                    tracing::debug!("Code Assist context resolution failed, falling back to standard API: {}", err);
                }
            }
        }

        let url = if auth.is_api_key() {
            if streaming {
                format!(
                    "{}/{}:{}&key={}",
                    BASE_URL,
                    model_name,
                    action,
                    auth.credential()
                )
            } else {
                format!(
                    "{}/{}:{}?key={}",
                    BASE_URL,
                    model_name,
                    action,
                    auth.credential()
                )
            }
        } else {
            format!("{}/{}:{}", BASE_URL, model_name, action)
        };
        Ok((url, self.build_request_body(request)?))
    }

    /// Build a Gemini generateContent request body.
    fn build_request_body(&self, request: &ChatRequest) -> Result<String> {
        let mut contents = Vec::new();
        let mut system_prompt: Option<String> = None;

        // Group consecutive messages by role for Gemini's strict alternation
        let mut grouped_messages: Vec<(String, Vec<serde_json::Value>)> = Vec::new();

        for msg in request.messages {
            if msg.role == "system" {
                system_prompt = Some(msg.content.clone());
                continue;
            }

            // Map roles: Gemini uses "user" and "model"
            let role = match msg.role.as_str() {
                "user" | "tool" => "user",
                "assistant" => "model",
                _ => "user",
            };

            let mut parts = Vec::new();

            // Handle native tool results (functionResponse)
            if msg.role == "tool" {
                if msg.tool_call_id.is_some() {
                    let part = json!({
                        "functionResponse": {
                            "name": msg.name.as_deref().unwrap_or("unknown"),
                            "response": {
                                "content": msg.content
                            }
                        }
                    });

                    parts.push(part);
                } else {
                    // Fallback for non-native tool results
                    parts.push(json!({"text": format!("[Tool Result]:\n{}", msg.content)}));
                }
            } else if let Some(tool_calls) = &msg.tool_calls {
                // Handle assistant native tool calls (functionCall)
                if !msg.content.is_empty() {
                    let text_part = json!({"text": msg.content});
                    parts.push(text_part);
                }
                for tc in tool_calls {
                    let fc_obj = json!({
                        "name": tc.function.name,
                        "args": serde_json::from_str::<serde_json::Value>(&tc.function.arguments).unwrap_or(json!({}))
                    });

                    let part = json!({
                        "functionCall": fc_obj
                    });

                    parts.push(part);
                }
            } else if let Some(content_parts) = msg.content_parts.as_ref() {
                // Multimodal support
                for part in content_parts {
                    match part {
                        ContentPart::Text(text) => {
                            let p = json!({"text": text});
                            parts.push(p);
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
                            parts.push(json!({"text": format!("[Image: {}]", url)}));
                        }
                    }
                }
            } else {
                let p = json!({"text": msg.content});
                parts.push(p);
            }

            if let Some(last) = grouped_messages.last_mut() {
                if last.0 == role {
                    last.1.extend(parts);
                } else {
                    grouped_messages.push((role.to_string(), parts));
                }
            } else {
                grouped_messages.push((role.to_string(), parts));
            }
        }

        for (role, parts) in grouped_messages {
            contents.push(json!({
                "role": role,
                "parts": parts
            }));
        }

        let mut generation_config = json!({
            "temperature": request.temperature,
            "maxOutputTokens": request.max_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
        });

        // Handle reasoning/thinking models
        if let Some(effort) = request.reasoning_effort
            && effort != "none" {
                let model_lower = request.model.to_lowercase();
                if model_lower.contains("gemini-2.0") || model_lower.contains("flash-thinking") {
                    // Newer Gemini models use thinkingConfig
                    generation_config["thinkingConfig"] = json!({
                        "includeThoughts": true,
                        // Mapping "low/medium/high" to budget tokens
                        "thinkingBudget": match effort {
                            "low" => 4096,
                            "medium" => 12288,
                            "high" => 32768,
                            _ => 16384,
                        }
                    });
                }
            }

        let mut body = json!({
            "contents": contents,
            "generationConfig": generation_config
        });

        if let Some(sys) = system_prompt {
            body["systemInstruction"] = json!({
                "parts": [{"text": sys}]
            });
        }

        // Add native tools if provided
        if let Some(tools) = request.tools {
            let gemini_tools: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "function_declarations": [{
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters
                        }]
                    })
                })
                .collect();
            body["tools"] = json!(gemini_tools);
        }

        Ok(body.to_string())
    }

    fn normalize_token_usage(usage: &mut TokenUsage) {
        if usage.total_tokens == 0 && (usage.prompt_tokens > 0 || usage.completion_tokens > 0) {
            usage.total_tokens = usage.prompt_tokens + usage.completion_tokens;
        }
        if usage.completion_tokens == 0 && usage.total_tokens > usage.prompt_tokens {
            usage.completion_tokens = usage.total_tokens - usage.prompt_tokens;
        }
    }

    fn parse_usage_metadata(v: &serde_json::Value) -> Option<TokenUsage> {
        let obj = v.as_object()?;
        let mut usage = TokenUsage::default();
        let mut found = false;

        if let Some(count) = obj.get("promptTokenCount").and_then(|c| c.as_u64()) {
            usage.prompt_tokens = count as u32;
            found = true;
        }
        if let Some(count) = obj.get("candidatesTokenCount").and_then(|c| c.as_u64()) {
            usage.completion_tokens = count as u32;
            found = true;
        }
        if let Some(count) = obj.get("totalTokenCount").and_then(|c| c.as_u64()) {
            usage.total_tokens = count as u32;
            found = true;
        }

        if !found {
            return None;
        }
        Self::normalize_token_usage(&mut usage);
        Some(usage)
    }

    /// Parse text content and tool calls from a Gemini generateContent response.
    fn parse_response(&self, body: &str) -> Result<ChatResponse> {
        let parsed: serde_json::Value =
            serde_json::from_str(body).context("Failed to parse Gemini response")?;

        if let Some(error) = parsed.get("error") {
            let msg = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown Gemini API error");
            let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
            anyhow::bail!("Gemini API error (code {}): {}", code, msg);
        }

        let response_root = parsed.get("response").unwrap_or(&parsed);

        if let Some(feedback) = response_root.get("promptFeedback")
            && let Some(block_reason) = feedback.get("blockReason") {
                let reason = block_reason.as_str().unwrap_or("unknown");
                anyhow::bail!("Gemini blocked the prompt: {}", reason);
            }

        let mut content = String::new();
        let mut reasoning_content = String::new();
        let mut tool_calls = Vec::new();
        let mut turn_thought_signature: Option<String> = None;

        if let Some(candidates) = response_root.get("candidates")
            && let Some(candidate) = candidates.get(0) {
                if let Some(finish_reason) = candidate.get("finish_reason") {
                    let reason = finish_reason.as_str().unwrap_or("unknown");
                    if reason != "STOP" && reason != "MAX_TOKENS" {
                        tracing::warn!("Gemini finish reason: {}", reason);
                    }
                }

                if let Some(cand_content) = candidate.get("content")
                    && let Some(parts) = cand_content.get("parts")
                    && let Some(parts_array) = parts.as_array() {
                        // First pass: find ANY thought_signature in the whole turn
                        for part in parts_array {
                            if let Some(sig) = part.get("thought_signature").and_then(|v| v.as_str()) {
                                turn_thought_signature = Some(sig.to_string());
                                break;
                            }
                        }

                        for part in parts_array {
                            // Support for "thought" (reasoning) parts in newer Gemini models
                            if let Some(thought) = part.get("thought").and_then(|t| t.as_str()) {
                                reasoning_content.push_str(thought);
                            } else if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                content.push_str(text);
                            }
                            if let Some(fc) = part.get("functionCall")
                                && let Some(name) = fc.get("name").and_then(|n| n.as_str()) {
                                    let args = fc.get("args").cloned().unwrap_or(json!({}));
                                    let call_id = format!("call_{}", uuid::Uuid::new_v4().simple());
                                    
                                    // thought_signature is a sibling to functionCall in the Part
                                    // If missing on this part, use the turn-level one we found
                                    let part_signature = part.get("thought_signature").and_then(|v| v.as_str()).map(|s| s.to_string());
                                    let final_signature = part_signature.or_else(|| turn_thought_signature.clone());

                                    tool_calls.push(crate::providers::ToolCall {
                                        id: call_id,
                                        kind: "function".to_string(),
                                        function: crate::providers::FunctionCall {
                                            name: name.to_string(),
                                            arguments: args.to_string(),
                                            thought_signature: final_signature,
                                        },
                                    });
                                }
                        }
                    }
            }

        let mut usage = response_root
            .get("usageMetadata")
            .and_then(Self::parse_usage_metadata)
            .unwrap_or_default();

        if usage.total_tokens == 0 {
            usage.prompt_tokens = 0; // Estimation fallback
            usage.completion_tokens = (content.len() as u32).div_ceil(4);
            usage.total_tokens = usage.completion_tokens;
        }

        Ok(ChatResponse {
            content: if content.is_empty() && tool_calls.is_empty() {
                None
            } else {
                Some(content)
            },
            tool_calls,
            usage,
            model: "gemini".to_string(),
            reasoning_content: if reasoning_content.is_empty() {
                None
            } else {
                Some(reasoning_content)
            },
            thought_signature: turn_thought_signature,
        })
    }
}


impl Provider for GeminiProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let mut auth = self.auth.clone().ok_or_else(|| {
            anyhow::anyhow!("No Gemini credentials configured.")
        })?;

        let (url, body) = self.build_request_target(request, &auth, false)?;

        let mut req_builder = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(request.timeout_secs))
            .header("Content-Type", "application/json");

        if !auth.is_api_key() {
            req_builder =
                req_builder.header("Authorization", format!("Bearer {}", auth.credential()));
        }

        let res = req_builder.body(body).send()?;

        if res.status() == reqwest::StatusCode::UNAUTHORIZED
            && matches!(auth, GeminiAuth::OAuthToken(_))
            && let Some(creds) = Self::try_load_gemini_cli_token() {
                auth = GeminiAuth::OAuthToken(creds.access_token);
                let (retry_url, body_retry) = self.build_request_target(request, &auth, false)?;

                let retry_builder = self
                    .client
                    .post(&retry_url)
                    .timeout(Duration::from_secs(request.timeout_secs))
                    .header("Content-Type", "application/json")
                    .header("Authorization", format!("Bearer {}", auth.credential()));

                let res_retry = retry_builder.body(body_retry).send()?;
                if !res_retry.status().is_success() {
                    let status = res_retry.status();
                    let text = res_retry.text().unwrap_or_default();
                    anyhow::bail!("Gemini API error after refresh {}: {}", status, text);
                }

                let resp_text = res_retry.text()?;
                return self.parse_response(&resp_text);
            }

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().unwrap_or_default();
            anyhow::bail!("Gemini API error {}: {}", status, text);
        }

        let resp_text = res.text()?;
        let mut response = self.parse_response(&resp_text)?;
        response.model = request.model.to_string();
        Ok(response)
    }

    fn supports_native_tools(&self) -> bool {
        true
    }

    fn get_name(&self) -> &str {
        "gemini"
    }

    fn chat_stream(
        &self,
        request: &ChatRequest,
        mut callback: StreamCallback,
    ) -> Result<ChatResponse> {
        use crate::providers::StreamChunk;
        use crate::providers::sse::SseReader;

        let auth = self
            .auth
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No Gemini credentials configured."))?;
        let (url, body) = self.build_request_target(request, &auth, true)?;

        let mut req_builder = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(request.timeout_secs))
            .header("Content-Type", "application/json");

        if !auth.is_api_key() {
            req_builder =
                req_builder.header("Authorization", format!("Bearer {}", auth.credential()));
        }

        let res = req_builder.body(body).send()?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().unwrap_or_default();
            anyhow::bail!("Gemini streaming API error {}: {}", status, text);
        }

        let mut sse_reader = SseReader::new(res);
        let mut full_content = String::new();
        let mut tool_calls = Vec::new();
        let mut turn_thought_signature: Option<String> = None;

        while let Some(data) = sse_reader.next_data() {
            if data == "[DONE]" {
                break;
            }

            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data) {
                let response_root = parsed.get("response").unwrap_or(&parsed);
                if let Some(candidates) = response_root.get("candidates")
                    && let Some(candidate) = candidates.get(0)
                        && let Some(cand_content) = candidate.get("content")
                            && let Some(parts) = cand_content.get("parts")
                            && let Some(parts_array) = parts.as_array() {
                                // Capture any thought_signature seen in the stream
                                for part in parts_array {
                                    if let Some(sig) = part.get("thought_signature").and_then(|ts| ts.as_str()) {
                                        turn_thought_signature = Some(sig.to_string());
                                    }
                                }

                                for part in parts_array {
                                    if let Some(thought) = part.get("thought").and_then(|t| t.as_str()) {
                                        // TODO: Should we pipe reasoning to callback differently?
                                        // For now, we collect it and it will be in the final response
                                        full_content.push_str(thought);
                                        // callback(StreamChunk::Delta(thought.to_string())); // Don't pipe reasoning to text delta yet
                                    } else if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                        full_content.push_str(text);
                                        callback(StreamChunk::Delta(text.to_string()));
                                    }
                                    if let Some(fc) = part.get("functionCall")
                                        && let Some(name) = fc.get("name").and_then(|n| n.as_str()) {
                                            let args = fc.get("args").cloned().unwrap_or(json!({}));
                                            let call_id = format!("call_{}", uuid::Uuid::new_v4().simple());
                                            
                                            let part_signature = part.get("thought_signature").and_then(|ts| ts.as_str()).map(|s| s.to_string());
                                            let final_signature = part_signature.or_else(|| turn_thought_signature.clone());

                                            let tc = crate::providers::ToolCall {
                                                id: call_id,
                                                kind: "function".to_string(),
                                                function: crate::providers::FunctionCall {
                                                    name: name.to_string(),
                                                    arguments: args.to_string(),
                                                    thought_signature: final_signature,
                                                },
                                            };
                                            tool_calls.push(tc);
                                        }
                                }
                            }
            }
        }

        let usage = TokenUsage {
            prompt_tokens: 0,
            completion_tokens: (full_content.len() as u32).div_ceil(4),
            total_tokens: (full_content.len() as u32).div_ceil(4),
        };

        callback(StreamChunk::Done(usage.clone()));

        Ok(ChatResponse {
            content: if full_content.is_empty() && tool_calls.is_empty() {
                None
            } else {
                Some(full_content)
            },
            tool_calls,
            usage,
            model: request.model.to_string(),
            reasoning_content: None,
            thought_signature: turn_thought_signature,
        })
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
        assert_eq!(GeminiAuth::ExplicitKey("k".to_string()).source(), "config");
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
    fn test_parse_oauth_client_from_js() {
        let content = r#"
            const clientId = "123456789-abcdef.apps.googleusercontent.com";
            const clientSecret = "GOCSPX-FakeSecretValue123";
        "#;
        let parsed = GeminiProvider::parse_oauth_client_from_js(content).unwrap();
        assert_eq!(parsed.0, "123456789-abcdef.apps.googleusercontent.com");
        assert_eq!(parsed.1, "GOCSPX-FakeSecretValue123");
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

    #[test]
    fn test_cli_oauth_sentinel_detection() {
        assert!(GeminiProvider::is_cli_oauth_sentinel("cli_oauth"));
        assert!(GeminiProvider::is_cli_oauth_sentinel("CLI_OAUTH"));
        assert!(!GeminiProvider::is_cli_oauth_sentinel("AIza..."));
    }

    #[test]
    fn test_env_oauth_uses_code_assist_endpoint() {
        let auth = GeminiAuth::EnvOAuthToken("ya29.token".to_string());
        assert!(GeminiProvider::uses_code_assist_endpoint(&auth));
    }

    #[test]
    fn test_parse_response_code_assist_wrapper() {
        let provider = GeminiProvider::new(None);
        let body = r#"{
            "response": {
                "candidates": [
                    {
                        "content": {
                            "parts": [{"text": "hello"}]
                        },
                        "finishReason": "STOP"
                    }
                ]
            }
        }"#;

        let parsed = provider.parse_response(body).unwrap();
        assert_eq!(parsed.content, Some("hello".to_string()));
    }

    #[test]
    fn test_parse_response_with_reasoning() {
        let provider = GeminiProvider::new(None);
        let body = r#"{
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {"thought": "I should say hello"},
                            {"text": "Hello!"}
                        ]
                    },
                    "finishReason": "STOP"
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 20,
                "totalTokenCount": 30
            }
        }"#;

        let parsed = provider.parse_response(body).unwrap();
        assert_eq!(parsed.content, Some("Hello!".to_string()));
        assert_eq!(parsed.reasoning_content, Some("I should say hello".to_string()));
        assert_eq!(parsed.usage.total_tokens, 30);
    }
}
