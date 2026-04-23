use crate::workspace_templates::*;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Legacy banner (used by main.rs)
// ─────────────────────────────────────────────────────────────────────────────

pub const BANNER: &str = concat!(
    "\n",
    "   ___                 ____\n",
    "  / _ \\ _ __   ___ _ __|  _ \\ __ ___      __\n",
    " | | | | '_ \\ / _ \\ '_ \\ |_) / _` \\ \\ /\\ / /\n",
    " | |_| | |_) |  __/ | | |  __/ (_| |\\ V  V /\n",
    "  \\___/| .__/ \\___|_| |_|_|   \\__,_| \\_/\\_/\n",
    "       |_|\n",
    "\n"
);

// ─────────────────────────────────────────────────────────────────────────────
// Public structs
// ─────────────────────────────────────────────────────────────────────────────

pub struct ProjectContext {
    pub user_name: String,
    pub timezone: String,
    pub agent_name: String,
    pub communication_style: String,
}

impl Default for ProjectContext {
    fn default() -> Self {
        Self {
            user_name: "User".to_string(),
            timezone: "UTC".to_string(),
            agent_name: "OpenPaw".to_string(),
            communication_style: "Be warm, natural, and clear. Avoid robotic phrasing.".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub name: String,
    pub default_model: String,
    pub base_url: Option<String>,
    pub model: Option<String>,
}

#[derive(Default)]
pub struct PartialConfig {
    pub provider: Option<ProviderConfig>,
    pub selected_default_model: Option<String>,
    pub api_key: Option<String>,
    pub timezone: Option<String>,
    pub telegram: Option<Option<(String, String)>>,
    pub whatsapp_native: Option<Option<(String, String)>>,
    pub email: Option<Option<(String, String, String, u16, String, u16)>>,
    pub memory_backend: Option<String>,
    pub groq_key: Option<String>,
    pub embed_provider: Option<Option<String>>,
    pub embed_key: Option<Option<String>>,
    pub embed_model: Option<Option<String>>,
    pub composio_enabled: Option<bool>,
    pub composio_api_key: Option<Option<String>>,
    pub composio_entity_id: Option<String>,
    pub kilocode_fallback_models: Option<Vec<String>>,
    pub brave_api_key: Option<Option<String>>,
    pub search_provider: Option<String>,
    pub pushover: Option<Option<(String, String)>>,
    pub custom_base_url: Option<Option<String>>,
    pub custom_model: Option<Option<String>>,
    pub cheap_provider: Option<Option<String>>,
    pub cheap_model: Option<Option<String>>,
    pub cheap_api_key: Option<Option<String>>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Main onboarding flow
// ─────────────────────────────────────────────────────────────────────────────

pub const COMMON_TIMEZONES: &[(&str, &str)] = &[
    ("UTC", "UTC / GMT+0"),
    ("America/New_York", "Eastern  (UTC-5/4)"),
    ("America/Chicago", "Central  (UTC-6/5)"),
    ("America/Denver", "Mountain (UTC-7/6)"),
    ("America/Los_Angeles", "Pacific  (UTC-8/7)"),
    ("America/Sao_Paulo", "Brazil   (UTC-3)"),
    ("Europe/London", "London   (UTC+0/1)"),
    ("Europe/Paris", "Paris    (UTC+1/2)"),
    ("Europe/Berlin", "Berlin   (UTC+1/2)"),
    ("Europe/Moscow", "Moscow   (UTC+3)"),
    ("Asia/Dubai", "Dubai    (UTC+4)"),
    ("Asia/Kolkata", "India    (UTC+5:30)"),
    ("Asia/Singapore", "Singapore(UTC+8)"),
    ("Asia/Tokyo", "Tokyo    (UTC+9)"),
    ("Asia/Shanghai", "Shanghai (UTC+8)"),
    ("Australia/Sydney", "Sydney   (UTC+10/11)"),
    ("Pacific/Auckland", "Auckland (UTC+12/13)"),
];

pub fn interactive_onboard<P: AsRef<Path>>(workspace_dir: P) -> Result<()> {
    let dir = workspace_dir.as_ref();
    let config_path = dir.join("config.json");
    let mut partial = PartialConfig::default();
    let is_edit = config_path.exists();

    if is_edit {
        if let Ok(content) = fs::read_to_string(&config_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                partial.default_values_from_json(&json);
            }
        }
    }

    let app = crate::tui_onboard::App::new(partial, is_edit);
    let (should_save, partial) = app.run()?;

    if should_save {
        println!("[openpaw] Saving configuration...");
        if !dir.exists() {
            fs::create_dir_all(dir).context("Failed to create workspace directory")?;
            println!("[openpaw] Created workspace directory: {}", dir.display());
        }
        let config_json = partial.to_json();
        let json_str = serde_json::to_string_pretty(&config_json)?;
        fs::write(&config_path, &json_str)?;
        println!("[openpaw] Written config.json: {}", config_path.display());
        let ctx = ProjectContext {
            timezone: partial.timezone.clone().unwrap_or_else(|| "UTC".to_string()),
            ..ProjectContext::default()
        };
        scaffold_workspace(dir, &ctx)?;
        println!("[openpaw] Workspace templates scaffolded in: {}", dir.display());
    } else {
        println!("[openpaw] Setup was cancelled or discarded — nothing saved.");
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// PartialConfig logic (preserved exactly)
// ─────────────────────────────────────────────────────────────────────────────

impl PartialConfig {
    pub fn default_values_from_json(&mut self, json: &serde_json::Value) {
        if let Some(p) = json["default_provider"].as_str() {
            self.provider = Some(ProviderConfig {
                name: p.to_string(),
                default_model: json["default_model"].as_str().unwrap_or("").to_string(),
                base_url: json["models"]["providers"][p]["base_url"]
                    .as_str()
                    .map(|s| s.to_string()),
                model: json["models"]["providers"][p]["model"]
                    .as_str()
                    .map(|s| s.to_string()),
            });
            self.selected_default_model = json["default_model"].as_str().map(|s| s.to_string());
            self.api_key = json["models"]["providers"][p]["api_key"]
                .as_str()
                .map(|s| s.to_string());
            if let Some(fallbacks) = json["models"]["providers"][p]["fallback_models"].as_array() {
                self.kilocode_fallback_models = Some(
                    fallbacks
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.to_string())
                        .collect(),
                );
            }
        }
        if let Some(agents_arr) = json["agents"].as_array() {
            if let Some(first_agent) = agents_arr.first() {
                if let Some(cp) = first_agent["cheap_provider"].as_str() {
                    self.cheap_provider = Some(Some(cp.to_string()));
                    self.cheap_api_key = Some(
                        json["models"]["providers"][cp]["api_key"]
                            .as_str()
                            .map(|s| s.to_string()),
                    );
                }
                if let Some(cm) = first_agent["cheap_model"].as_str() {
                    self.cheap_model = Some(Some(cm.to_string()));
                }
            }
        }
        if let Some(tz) = json["timezone"].as_str() {
            self.timezone = Some(tz.to_string());
        }
        if let Some(backend) = json["memory"]["backend"].as_str() {
            self.memory_backend = Some(backend.to_string());
        }
        if let Some(voice) = json.get("voice") {
            self.groq_key = voice["api_key"].as_str().map(|s| s.to_string());
        }
        if let Some(tg_list) = json["channels"]["telegram"].as_array() {
            if !tg_list.is_empty() {
                let tg = &tg_list[0];
                let token = tg["bot_token"].as_str().unwrap_or("").to_string();
                let allow = tg["allow_from"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                self.telegram = Some(Some((token, allow)));
            } else {
                self.telegram = Some(None);
            }
        }
        if let Some(comp) = json.get("composio") {
            self.composio_enabled = comp["enabled"].as_bool();
            self.composio_api_key = Some(comp["api_key"].as_str().map(|s| s.to_string()));
            self.composio_entity_id = comp["entity_id"].as_str().map(|s| s.to_string());
        }
        if let Some(http) = json.get("http_request") {
            self.brave_api_key = Some(http["brave_search_api_key"].as_str().map(|s| s.to_string()));
        }
        if let Some(wa_list) = json["channels"]["whatsapp_native"].as_array() {
            if !wa_list.is_empty() {
                let wa = &wa_list[0];
                let url = wa["bridge_url"].as_str().unwrap_or("").to_string();
                let allow = wa["allow_from"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                self.whatsapp_native = Some(Some((url, allow)));
            } else {
                self.whatsapp_native = Some(None);
            }
        }
        if let Some(push) = json.get("pushover") {
            if push["enabled"].as_bool().unwrap_or(false) {
                let t = push["token"].as_str().unwrap_or("").to_string();
                let u = push["user_key"].as_str().unwrap_or("").to_string();
                self.pushover = Some(Some((t, u)));
            } else {
                self.pushover = Some(None);
            }
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;

        let p_name = self
            .provider
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "gemini".to_string());
        let mut providers = json!({
            &p_name: { "api_key": self.api_key.as_ref().cloned().unwrap_or_default() }
        });
        let provider_model = self
            .provider
            .as_ref()
            .map(|p| p.model.clone().unwrap_or_else(|| p.default_model.clone()))
            .or_else(|| self.selected_default_model.clone())
            .unwrap_or_else(|| "gemini-2.0-flash".to_string());
        providers[&p_name]["model"] = json!(provider_model);
        if let Some(Some(url)) = &self.custom_base_url {
            providers[&p_name]["base_url"] = json!(url);
        } else if let Some(url) = self.provider.as_ref().and_then(|p| p.base_url.as_ref()) {
            providers[&p_name]["base_url"] = json!(url);
        }
        if let Some(fallbacks) = &self.kilocode_fallback_models {
            providers[&p_name]["fallback_models"] = json!(fallbacks);
        }
        if let Some(Some(ep)) = &self.embed_provider {
            if ep != &p_name {
                providers[ep] = json!({
                    "api_key": self.embed_key.as_ref().and_then(|k| k.as_ref()).cloned().unwrap_or_default()
                });
            }
        }
        if let Some(Some(cp)) = &self.cheap_provider {
            if cp != &p_name
                && self
                    .cheap_api_key
                    .as_ref()
                    .map(|k| k.is_some())
                    .unwrap_or(false)
            {
                providers[cp] = json!({
                    "api_key": self.cheap_api_key.as_ref().unwrap().as_ref().unwrap().clone()
                });
            }
        }

        let telegram_vec = match &self.telegram {
            Some(Some((token, user))) => json!([{
                "account_id": "main", "bot_token": token,
                "allow_from": [user], "group_policy": "allowlist"
            }]),
            _ => json!([]),
        };
        let whatsapp_vec = match &self.whatsapp_native {
            Some(Some((url, phone))) => json!([{
                "account_id": "main", "bridge_url": url,
                "allow_from": [phone], "auto_start": true
            }]),
            _ => json!([]),
        };

        let email_vec = match &self.email {
            Some(Some((email, pass, smtp_host, smtp_port, imap_host, imap_port))) => json!([{
                "account_id": "main",
                "smtp_user": email,
                "smtp_pass": pass,
                "smtp_host": smtp_host,
                "smtp_port": smtp_port,
                "imap_host": imap_host,
                "imap_port": imap_port
            }]),
            _ => json!([]),
        };

        let mut config = json!({
            "default_provider": p_name,
            "default_model": self.selected_default_model.as_ref().cloned().unwrap_or_else(|| "gemini-2.0-flash".to_string()),
            "timezone": self.timezone.as_ref().cloned().unwrap_or_else(|| "UTC".to_string()),
            "models": { "providers": providers },
            "agents": [{
                "name": "default",
                "provider": p_name,
                "model": self.selected_default_model.as_ref().cloned().unwrap_or_else(|| "gemini-2.0-flash".to_string()),
                "cheap_provider": self.cheap_provider.as_ref().and_then(|p| p.as_ref()).cloned(),
                "cheap_model": self.cheap_model.as_ref().and_then(|m| m.as_ref()).cloned()
            }],
            "channels": { "telegram": telegram_vec, "whatsapp_native": whatsapp_vec, "email": email_vec },
            "memory": {
                "backend": self.memory_backend.as_ref().cloned().unwrap_or_else(|| "sqlite".to_string()),
                "embedding_provider": self.embed_provider.as_ref().and_then(|ep| ep.as_ref()).cloned()
            },
            "composio": {
                "enabled": self.composio_enabled.unwrap_or(false),
                "api_key": self.composio_api_key.as_ref().and_then(|k| k.as_ref()).cloned(),
                "entity_id": self.composio_entity_id.as_ref().cloned().unwrap_or_else(|| "default".to_string())
            },
            "http_request": {
                "enabled": true,
                "search_provider": self.search_provider.as_deref().unwrap_or_else(|| {
                    if self.brave_api_key.as_ref().and_then(|k| k.as_ref()).is_some() { "brave" } else { "gemini_cli" }
                }),
                "brave_search_api_key": self.brave_api_key.as_ref().and_then(|k| k.as_ref()).cloned()
            },
            "pushover": {
                "enabled": self.pushover.as_ref().and_then(|p| p.as_ref()).is_some(),
                "token":    self.pushover.as_ref().and_then(|p| p.as_ref()).map(|p| p.0.clone()),
                "user_key": self.pushover.as_ref().and_then(|p| p.as_ref()).map(|p| p.1.clone())
            }
        });

        if let Some(Some(m)) = &self.embed_model {
            config["memory"]["embedding_model"] = json!(m);
        }
        if let Some(key) = &self.groq_key {
            config["voice"] = json!({
                "provider": "groq", "api_key": key, "model": "whisper-large-v3"
            });
        }
        config
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Workspace scaffolding
// ─────────────────────────────────────────────────────────────────────────────

pub fn scaffold_workspace<P: AsRef<Path>>(workspace_dir: P, ctx: &ProjectContext) -> Result<()> {
    let dir = workspace_dir.as_ref();
    if !dir.exists() {
        fs::create_dir_all(dir).context("Failed to create workspace directory")?;
    }
    let soul = SOUL_TEMPLATE
        .replace("{{agent_name}}", &ctx.agent_name)
        .replace("{{communication_style}}", &ctx.communication_style);
    let identity = IDENTITY_TEMPLATE.replace("{{agent_name}}", &ctx.agent_name);
    let user = USER_TEMPLATE
        .replace("{{user_name}}", &ctx.user_name)
        .replace("{{timezone}}", &ctx.timezone);

    write_if_missing(&dir.join("SOUL.md"), &soul)?;
    write_if_missing(&dir.join("AGENTS.md"), AGENTS_TEMPLATE)?;
    write_if_missing(&dir.join("TOOLS.md"), TOOLS_TEMPLATE)?;
    write_if_missing(&dir.join("IDENTITY.md"), &identity)?;
    write_if_missing(&dir.join("USER.md"), &user)?;
    write_if_missing(&dir.join("MEMORY.md"), MEMORY_TEMPLATE)?;
    write_if_missing(&dir.join("HEARTBEAT.md"), HEARTBEAT_TEMPLATE)?;
    write_if_missing(&dir.join("BOOTSTRAP.md"), BOOTSTRAP_TEMPLATE)?;
    Ok(())
}

fn write_if_missing(path: &Path, content: &str) -> Result<()> {
    if !path.exists() {
        fs::write(path, content).context(format!("Failed to write {}", path.display()))?;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Dynamic model fetching (used by TUI)
// ─────────────────────────────────────────────────────────────────────────────

pub fn fetch_ollama_models() -> Result<Vec<String>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?;
    let res = client.get("http://localhost:11434/api/tags").send()?;
    if !res.status().is_success() {
        anyhow::bail!("Ollama error {}", res.status());
    }
    let payload: serde_json::Value = res.json()?;
    Ok(payload["models"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|m| m["name"].as_str().map(|s| s.to_string()))
        .collect())
}

pub fn fetch_openai_compat_models(base_url: &str, api_key: &str) -> Result<Vec<String>> {
    use std::time::Duration;
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()?;
    let mut req = client.get(&url).header("Accept", "application/json");
    if !api_key.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
    }
    let res = req.send()?;
    if !res.status().is_success() {
        anyhow::bail!("models error {}", res.status());
    }
    let payload: serde_json::Value = res.json()?;
    let data = payload["data"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing 'data' array"))?;
    let mut models: Vec<String> = data
        .iter()
        .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
        .collect();
    models.sort();
    models.dedup();
    Ok(models)
}

pub fn fetch_ollama_compat_models(base_url: &str) -> Result<Vec<String>> {
    fetch_openai_compat_models(base_url, "")
}

pub fn fetch_gemini_models(api_key: &str) -> Result<Vec<String>> {
    use std::time::Duration;
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models?key={}",
        urlencoding::encode(api_key.trim())
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let res = client.get(&url).send()?;
    if !res.status().is_success() {
        anyhow::bail!("Gemini models error {}", res.status());
    }
    let payload: serde_json::Value = res.json()?;
    let models_arr = payload["models"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing 'models' array"))?;

    let mut models: Vec<String> = models_arr
        .iter()
        .filter_map(|m| {
            let name = m["name"].as_str()?;
            let methods = m["supportedGenerationMethods"].as_array()?;
            let ok = methods
                .iter()
                .any(|v| v.as_str() == Some("generateContent"));
            if ok {
                Some(name.trim_start_matches("models/").to_string())
            } else {
                None
            }
        })
        .collect();

    models.sort_by(|a, b| b.cmp(a));
    Ok(models)
}

pub fn fetch_openai_models(api_key: &str) -> Result<Vec<String>> {
    use std::time::Duration;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let res = client
        .get("https://api.openai.com/v1/models")
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .send()?;
    if !res.status().is_success() {
        anyhow::bail!("OpenAI models error {}", res.status());
    }
    let payload: serde_json::Value = res.json()?;
    let data = payload["data"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing 'data' array"))?;

    let mut models: Vec<(String, i64)> = data
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            let created = m["created"].as_i64().unwrap_or(0);
            let lower = id.to_lowercase();
            if lower.starts_with("gpt-")
                || lower.starts_with("o1")
                || lower.starts_with("o3")
                || lower.starts_with("o4")
                || lower.starts_with("chatgpt-")
            {
                Some((id, created))
            } else {
                None
            }
        })
        .collect();

    models.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(models.into_iter().map(|(id, _)| id).collect())
}

pub fn fetch_anthropic_models(api_key: &str) -> Result<Vec<String>> {
    use std::time::Duration;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let res = client
        .get("https://api.anthropic.com/v1/models")
        .header("x-api-key", api_key.trim())
        .header("anthropic-version", "2023-06-01")
        .send()?;
    if !res.status().is_success() {
        anyhow::bail!("Anthropic models error {}", res.status());
    }
    let payload: serde_json::Value = res.json()?;
    let data = payload["data"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing 'data' array"))?;

    let mut models: Vec<(String, String)> = data
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            let created = m["created_at"].as_str().unwrap_or("").to_string();
            Some((id, created))
        })
        .collect();

    models.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(models.into_iter().map(|(id, _)| id).collect())
}
