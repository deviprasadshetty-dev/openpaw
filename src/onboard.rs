use crate::providers::kilocode::{
    fetch_kilocode_free_models, preferred_kilocode_model_index,
};
use crate::workspace_templates::*;
use anyhow::{Context, Result};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Legacy banner (used by main.rs when no command is provided).
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

// ── ANSI colour helpers ───────────────────────────────────────────

macro_rules! ansi {
    ($code:expr, $s:expr) => {
        format!("\x1b[{}m{}\x1b[0m", $code, $s)
    };
}

fn bold(s: &str) -> String {
    ansi!("1", s)
}
fn dim(s: &str) -> String {
    ansi!("2", s)
}
fn green(s: &str) -> String {
    ansi!("1;32", s)
}
fn cyan(s: &str) -> String {
    ansi!("1;36", s)
}
fn yellow(s: &str) -> String {
    ansi!("1;33", s)
}
fn magenta(s: &str) -> String {
    ansi!("1;35", s)
}
fn blue(s: &str) -> String {
    ansi!("1;34", s)
}

fn dim_cyan(s: &str) -> String {
    ansi!("2;36", s)
}

fn ok() -> String {
    green("✓")
}
fn skip() -> String {
    dim("·")
}
fn warn_icon() -> String {
    yellow("⚠")
}
fn info() -> String {
    dim_cyan("ℹ")
}

// ── Public structs ────────────────────────────────────────────────

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
struct PartialConfig {
    provider: Option<ProviderConfig>,
    selected_default_model: Option<String>,
    api_key: Option<String>,
    telegram: Option<Option<(String, String)>>,
    whatsapp_native: Option<Option<(String, String)>>,
    memory_backend: Option<String>,
    groq_key: Option<String>,
    embed_provider: Option<Option<String>>,
    embed_key: Option<Option<String>>,
    embed_model: Option<Option<String>>,
    composio_enabled: Option<bool>,
    composio_api_key: Option<Option<String>>,
    composio_entity_id: Option<String>,
    kilocode_fallback_models: Option<Vec<String>>,
    brave_api_key: Option<Option<String>>,
    pushover: Option<Option<(String, String)>>,
    custom_base_url: Option<Option<String>>,
    custom_model: Option<Option<String>>,
}

// ── Main onboarding flow ──────────────────────────────────────────

pub fn interactive_onboard<P: AsRef<Path>>(workspace_dir: P) -> Result<()> {
    let dir = workspace_dir.as_ref();

    // ── Banner ───────────────────────────────────────────────────────
    println!();
    println!(
        "{}",
        cyan("  ╭───────────────────────────────────────────────────────╮")
    );
    println!(
        "{}",
        cyan("  │")
            + &magenta("   ██████╗ ██████╗ ███████╗███╗   ██╗██████╗  █████╗ ██╗    ██╗")
            + &cyan("  │")
    );
    println!(
        "{}",
        cyan("  │")
            + &magenta("  ██╔═══██╗██╔══██╗██╔════╝████╗  ██║██╔══██╗██╔══██╗██║    ██║")
            + &cyan("  │")
    );
    println!(
        "{}",
        cyan("  │")
            + &cyan("  ██║   ██║██████╔╝█████╗  ██╔██╗ ██║██████╔╝███████║██║ █╗ ██║")
            + &cyan("  │")
    );
    println!(
        "{}",
        cyan("  │")
            + &dim("  ██║   ██║██╔═══╝ ██╔══╝  ██║╚██╗██║██╔═══╝ ██╔══██║██║███╗██║")
            + &cyan("  │")
    );
    println!(
        "{}",
        cyan("  │")
            + &bold("  ╚██████╔╝██║     ███████╗██║ ╚████║██║     ██║  ██║╚███╔███╔╝")
            + &cyan("  │")
    );
    println!(
        "{}",
        cyan("  │")
            + &dim("   ╚═════╝ ╚═╝     ╚══════╝╚═╝  ╚═══╝╚═╝     ╚═╝  ╚═╝ ╚══╝╚══╝")
            + &cyan("  │")
    );
    println!(
        "{}",
        cyan("  │")
            + &dim("                                                               ")
            + &cyan("  │")
    );
    println!(
        "{}",
        cyan("  │")
            + &bold("     Your lightweight local AI agent  ·  Powered by Rust 🦀    ")
            + &cyan("  │")
    );
    println!(
        "{}",
        cyan("  ╰───────────────────────────────────────────────────────╯")
    );
    println!();

    let mut current_partial = PartialConfig::default();
    let config_path = dir.join("config.json");
    let mut is_edit = false;

    if config_path.exists() {
        println!("  {}  Existing configuration found.", info());
        println!("    {}  Edit existing configuration", cyan("1"));
        println!("    {}  Start fresh (overwrite)", cyan("2"));
        println!("    {}  Cancel", cyan("3"));
        println!();
        let choice = prompt_choice("  Action", 3, 1);
        if choice == 3 {
            return Ok(());
        }
        if choice == 1 {
            is_edit = true;
            // Load existing values into current_partial
            if let Ok(content) = fs::read_to_string(&config_path) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    current_partial.default_values_from_json(&json);
                }
            }
        }
    }

    if is_edit {
        loop {
            println!("\n  {}  {} What would you like to edit?", info(), bold("Edit Mode"));
            println!("    {}  AI Provider & Model", cyan("1"));
            println!("    {}  Memory & Embeddings", cyan("2"));
            println!("    {}  Voice (Groq Whisper)", cyan("3"));
            println!("    {}  Telegram Bot", cyan("4"));
            println!("    {}  Composio Tools", cyan("5"));
            println!("    {}  Web Search (Brave)", cyan("6"));
            println!("    {}  WhatsApp Native", cyan("7"));
            println!("    {}  Pushover", cyan("8"));
            println!("    {}  Save and Exit", green("9"));
            println!("    {}  Discard and Exit", yellow("10"));
            println!();

            let edit_choice = prompt_choice("  Choice", 10, 9);
            match edit_choice {
                1 => current_partial.step_provider()?,
                2 => current_partial.step_memory()?,
                3 => current_partial.step_voice()?,
                4 => current_partial.step_telegram()?,
                5 => current_partial.step_composio()?,
                6 => current_partial.step_brave()?,
                7 => current_partial.step_whatsapp()?,
                8 => current_partial.step_pushover()?,
                9 => break,
                10 => return Ok(()),
                _ => break,
            }
        }
    } else {
        // Full sequence
        current_partial.step_provider()?;
        current_partial.step_memory()?;
        current_partial.step_voice()?;
        current_partial.step_telegram()?;
        current_partial.step_composio()?;
        current_partial.step_brave()?;
        current_partial.step_whatsapp()?;
        current_partial.step_pushover()?;
    }

    // ── Write files ──────────────────────────────────────────────────
    if !dir.exists() {
        fs::create_dir_all(dir).context("Failed to create workspace directory")?;
    }

    // Prepare final config by merging partial with defaults
    let config_json = current_partial.to_json();
    fs::write(&config_path, serde_json::to_string_pretty(&config_json)?)?;
    scaffold_workspace(dir, &ProjectContext::default())?;

    // ── Done! ────────────────────────────────────────────────────────
    println!();
    println!(
        "{}",
        cyan("  ╭───────────────────────────────────────────────────────╮")
    );
    println!(
        "{}  {}  {}",
        cyan("  │"),
        green("✅  Configuration updated!"),
        cyan("                                      │")
    );
    println!(
        "{}",
        cyan("  ╰───────────────────────────────────────────────────────╯")
    );
    println!();

    let workspace_path = dir
        .canonicalize()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| dir.display().to_string());
    println!("  {}  {}", dim("Path"), cyan(&workspace_path));
    println!();
    
    // Summary
    println!("  {}  Next steps:", bold("→"));
    println!(
        "     {}  Run  {}  for interactive chat",
        dim("1."),
        green("openpaw agent")
    );
    println!();

    Ok(())
}

impl PartialConfig {
    fn default_values_from_json(&mut self, json: &serde_json::Value) {
        if let Some(p) = json["default_provider"].as_str() {
            self.provider = Some(ProviderConfig {
                name: p.to_string(),
                default_model: json["default_model"].as_str().unwrap_or("").to_string(),
                base_url: json["models"]["providers"][p]["base_url"].as_str().map(|s| s.to_string()),
                model: json["models"]["providers"][p]["model"].as_str().map(|s| s.to_string()),
            });
            self.selected_default_model = json["default_model"].as_str().map(|s| s.to_string());
            self.api_key = json["models"]["providers"][p]["api_key"].as_str().map(|s| s.to_string());
            
            if let Some(fallbacks) = json["models"]["providers"][p]["fallback_models"].as_array() {
                self.kilocode_fallback_models = Some(fallbacks.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect());
            }
        }

        if let Some(backend) = json["memory"]["backend"].as_str() {
            self.memory_backend = Some(backend.to_string());
        }

        if let Some(voice) = json.get("voice") {
             self.groq_key = voice["api_key"].as_str().map(|s| s.to_string());
        }

        // Telegram (it's a vec in config)
        if let Some(tg_list) = json["channels"]["telegram"].as_array() {
            if !tg_list.is_empty() {
                let tg = &tg_list[0];
                let token = tg["bot_token"].as_str().unwrap_or("").to_string();
                let allow = tg["allow_from"].as_array().and_then(|a| a.first()).and_then(|v| v.as_str()).unwrap_or("").to_string();
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
                let allow = wa["allow_from"].as_array().and_then(|a| a.first()).and_then(|v| v.as_str()).unwrap_or("").to_string();
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

    fn step_provider(&mut self) -> Result<()> {
        section_header(1, "AI Provider", "Which AI service should power your agent?");
        
        let providers = vec![
            ProviderConfig {
                name: "gemini".to_string(),
                default_model: "gemini-2.0-flash".to_string(),
                base_url: None,
                model: None,
            },
            ProviderConfig {
                name: "openai".to_string(),
                default_model: "gpt-4o".to_string(),
                base_url: None,
                model: None,
            },
            ProviderConfig {
                name: "anthropic".to_string(),
                default_model: "claude-3-5-sonnet-latest".to_string(),
                base_url: None,
                model: None,
            },
            ProviderConfig {
                name: "openrouter".to_string(),
                default_model: "deepseek/deepseek-chat-v3-0324:free".to_string(),
                base_url: Some("https://openrouter.ai/api/v1".to_string()),
                model: None,
            },
            ProviderConfig {
                name: "opencode".to_string(),
                default_model: "minimax-m2.5-free".to_string(),
                base_url: Some("https://opencode.ai/zen/v1".to_string()),
                model: None,
            },
            ProviderConfig {
                name: "kilocode".to_string(),
                default_model: "minimax/minimax-m2.1:free".to_string(),
                base_url: Some("https://api.kilo.ai/api/gateway".to_string()),
                model: None,
            },
            ProviderConfig {
                name: "ollama".to_string(),
                default_model: "llama3.2".to_string(),
                base_url: Some("http://localhost:11434/v1".to_string()),
                model: None,
            },
            ProviderConfig {
                name: "lmstudio".to_string(),
                default_model: "local-model".to_string(),
                base_url: Some("http://localhost:1234/v1".to_string()),
                model: None,
            },
            ProviderConfig {
                name: "openai-compatible".to_string(),
                default_model: "gpt-4o".to_string(),
                base_url: Some("http://localhost:8080/v1".to_string()),
                model: None,
            },
        ];

        let default_prov_idx = self.provider.as_ref().and_then(|p| providers.iter().position(|x| x.name == p.name)).unwrap_or(0);
        
        let provider_idx = prompt_choice("  Select provider", providers.len(), default_prov_idx + 1) - 1;
        let provider = providers[provider_idx].clone();

        // API Key
        let mut key = if provider.name == "ollama" || provider.name == "lmstudio" {
            String::new()
        } else if provider.name == "gemini" {
            println!("  How would you like to authenticate?");
            println!("    {}  API Key", cyan("1"));
            println!("    {}  Gemini CLI OAuth", cyan("2"));
            let auth = prompt_choice("  Choice", 2, 1);
            if auth == 2 {
                 "cli_oauth".to_string()
            } else {
                prompt_secret(&format!("  {} API key", bold("Gemini")))?
            }
        } else if provider.name == "openai-compatible" {
            String::new()
        } else {
            prompt_secret(&format!("  {} API key", bold(&capitalize(&provider.name))))?
        };

        let mut custom_base_url = provider.base_url.clone();
        let mut custom_model = provider.model.clone();

        if provider.name == "openai-compatible" {
            custom_base_url = Some(prompt_with_default("  Base URL", provider.base_url.as_deref().unwrap_or("http://localhost:8080/v1"))?);
            custom_model = Some(prompt_with_default("  Model Name", &provider.default_model)?);
            key = prompt_secret("  Custom Provider API key")?;
        }

        // Model discovery
        let mut selected_model = custom_model.clone().unwrap_or_else(|| provider.default_model.clone());
        
        if provider.name == "ollama" {
            if let Ok(models) = fetch_ollama_models() {
                if !models.is_empty() {
                    let d_idx = models.iter().position(|m| m.contains("qwen")).unwrap_or(0);
                    list_models_simple(&models, d_idx);
                    let c = prompt_choice("  Select model", models.len(), d_idx + 1) - 1;
                    selected_model = models[c].clone();
                }
            }
        } else if provider.name == "kilocode" {
             if let Ok(free_models) = fetch_kilocode_free_models(&key) {
                 if !free_models.is_empty() {
                     let d_idx = preferred_kilocode_model_index(&free_models);
                     list_models_simple(&free_models, d_idx);
                     let c = prompt_choice("  Select model", free_models.len(), d_idx + 1) - 1;
                     selected_model = free_models[c].clone();
                     self.kilocode_fallback_models = Some(free_models);
                 }
             }
        }

        self.provider = Some(provider);
        self.api_key = Some(key);
        self.selected_default_model = Some(selected_model);
        self.custom_base_url = Some(custom_base_url);
        self.custom_model = Some(custom_model);
        Ok(())
    }

    fn step_memory(&mut self) -> Result<()> {
        section_header(2, "Memory", "How should your agent remember things?");
        println!("    {}  sqlite (Recommended)", cyan("1"));
        println!("    {}  markdown", cyan("2"));
        println!("    {}  none", cyan("3"));
        let choice = prompt_choice("  Choice", 3, 1);
        self.memory_backend = Some(match choice {
            1 => "sqlite",
            2 => "markdown",
            _ => "none",
        }.to_string());

        if self.memory_backend.as_deref() == Some("sqlite") {
             print!("  Enable embeddings? (y/N): ");
             io::stdout().flush()?;
             let mut input = String::new();
             io::stdin().read_line(&mut input)?;
             if input.trim().eq_ignore_ascii_case("y") {
                 self.embed_provider = Some(Some("huggingface".to_string()));
                 self.embed_model = Some(Some("Qwen/Qwen3-Embedding-0.6B".to_string()));
                 self.embed_key = Some(Some("".to_string()));
             }
        }
        Ok(())
    }

    fn step_voice(&mut self) -> Result<()> {
        section_header(3, "Voice", "Groq Whisper Transcription");
        print!("  Enable voice? (y/N): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim().eq_ignore_ascii_case("y") {
            self.groq_key = Some(prompt_secret("  Groq API key")?);
        } else {
            self.groq_key = None;
        }
        Ok(())
    }

    fn step_telegram(&mut self) -> Result<()> {
        section_header(4, "Telegram", "Connect Telegram Bot");
        print!("  Set up Telegram? (y/N): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim().eq_ignore_ascii_case("y") {
            let token = prompt_secret("  Bot token")?;
            let user = prompt_with_default("  Username", "@you")?;
            self.telegram = Some(Some((token, user)));
        } else {
            self.telegram = Some(None);
        }
        Ok(())
    }

    fn step_composio(&mut self) -> Result<()> {
        section_header(5, "Composio", "Connect external apps");
        print!("  Enable Composio? (y/N): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim().eq_ignore_ascii_case("y") {
            self.composio_enabled = Some(true);
            self.composio_api_key = Some(Some(prompt_secret("  Composio API key")?));
            self.composio_entity_id = Some(prompt_with_default("  Entity ID", "default")?);
        } else {
            self.composio_enabled = Some(false);
        }
        Ok(())
    }

    fn step_brave(&mut self) -> Result<()> {
        section_header(6, "Web Search", "Brave Search API");
        print!("  Enable Brave Search? (y/N): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim().eq_ignore_ascii_case("y") {
            self.brave_api_key = Some(Some(prompt_secret("  Brave API key")?));
        } else {
            self.brave_api_key = Some(None);
        }
        Ok(())
    }

    fn step_whatsapp(&mut self) -> Result<()> {
        section_header(7, "WhatsApp", "WhatsApp Native Bridge");
        print!("  Enable WhatsApp? (y/N): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim().eq_ignore_ascii_case("y") {
            let phone = prompt_with_default("  Phone Number", "+1...")?;
            self.whatsapp_native = Some(Some(("http://localhost:18790".to_string(), phone)));
        } else {
            self.whatsapp_native = Some(None);
        }
        Ok(())
    }

    fn step_pushover(&mut self) -> Result<()> {
        section_header(8, "Pushover", "Push Notifications");
        print!("  Enable Pushover? (y/N): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim().eq_ignore_ascii_case("y") {
            let token = prompt_secret("  Token")?;
            let user = prompt_secret("  User Key")?;
            self.pushover = Some(Some((token, user)));
        } else {
            self.pushover = Some(None);
        }
        Ok(())
    }

    fn to_json(&self) -> serde_json::Value {
        use serde_json::json;

        let p_name = self.provider.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "gemini".to_string());
        let mut providers = json!({
            &p_name: {
                "api_key": self.api_key.as_ref().cloned().unwrap_or_default(),
            }
        });

        if let Some(Some(m)) = &self.custom_model {
            providers[&p_name]["model"] = json!(m);
        }
        if let Some(Some(url)) = &self.custom_base_url {
            providers[&p_name]["base_url"] = json!(url);
        }
        if let Some(fallbacks) = &self.kilocode_fallback_models {
            providers[&p_name]["fallback_models"] = json!(fallbacks);
        }

        if let Some(Some(ep)) = &self.embed_provider && ep != &p_name {
            providers[ep] = json!({ "api_key": self.embed_key.as_ref().and_then(|k| k.as_ref()).cloned().unwrap_or_default() });
        }

        let telegram_vec = match &self.telegram {
            Some(Some((token, user))) => json!([{
                "account_id": "main",
                "bot_token": token,
                "allow_from": [user],
                "group_policy": "allowlist"
            }]),
            _ => json!([]),
        };

        let whatsapp_vec = match &self.whatsapp_native {
            Some(Some((url, phone))) => json!([{
                "account_id": "main",
                "bridge_url": url,
                "allow_from": [phone],
                "auto_start": true
            }]),
            _ => json!([]),
        };

        let mut config = json!({
            "default_provider": p_name,
            "default_model": self.selected_default_model.as_ref().cloned().unwrap_or_else(|| "gemini-2.0-flash".to_string()),
            "models": { "providers": providers },
            "channels": {
                "telegram": telegram_vec,
                "whatsapp_native": whatsapp_vec
            },
            "memory": { 
                "backend": self.memory_backend.as_ref().cloned().unwrap_or_else(|| "sqlite".to_string()) 
            },
            "composio": {
                "enabled": self.composio_enabled.unwrap_or(false),
                "api_key": self.composio_api_key.as_ref().and_then(|k| k.as_ref()).cloned(),
                "entity_id": self.composio_entity_id.as_ref().cloned().unwrap_or_else(|| "default".to_string())
            },
            "http_request": {
                "enabled": true,
                "search_provider": if self.brave_api_key.as_ref().and_then(|k| k.as_ref()).is_some() { "brave" } else { "duckduckgo" },
                "brave_search_api_key": self.brave_api_key.as_ref().and_then(|k| k.as_ref()).cloned()
            },
            "pushover": {
                "enabled": self.pushover.as_ref().and_then(|p| p.as_ref()).is_some(),
                "token": self.pushover.as_ref().and_then(|p| p.as_ref()).map(|p| p.0.clone()),
                "user_key": self.pushover.as_ref().and_then(|p| p.as_ref()).map(|p| p.1.clone())
            }
        });

        if let Some(Some(m)) = &self.embed_model {
            config["memory"]["embedding_model"] = json!(m);
        }

        if let Some(key) = &self.groq_key {
            config["voice"] = json!({
                "provider": "groq",
                "api_key": key,
                "model": "whisper-large-v3"
            });
        }

        config
    }
}

// ── Workspace scaffolding ─────────────────────────────────────────

pub fn scaffold_workspace<P: AsRef<Path>>(workspace_dir: P, ctx: &ProjectContext) -> Result<()> {
    let dir = workspace_dir.as_ref();
    if !dir.exists() {
        fs::create_dir_all(dir).context("Failed to create workspace directory")?;
    }

    let soul_content = SOUL_TEMPLATE
        .replace("{{agent_name}}", &ctx.agent_name)
        .replace("{{communication_style}}", &ctx.communication_style);

    let identity_content = IDENTITY_TEMPLATE.replace("{{agent_name}}", &ctx.agent_name);

    let user_content = USER_TEMPLATE
        .replace("{{user_name}}", &ctx.user_name)
        .replace("{{timezone}}", &ctx.timezone);

    write_if_missing(&dir.join("SOUL.md"), &soul_content)?;
    write_if_missing(&dir.join("AGENTS.md"), AGENTS_TEMPLATE)?;
    write_if_missing(&dir.join("TOOLS.md"), TOOLS_TEMPLATE)?;
    write_if_missing(&dir.join("IDENTITY.md"), &identity_content)?;
    write_if_missing(&dir.join("USER.md"), &user_content)?;
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

fn fetch_ollama_models() -> Result<Vec<String>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?;
    let res = client.get("http://localhost:11434/api/tags").send()?;
    if !res.status().is_success() {
        anyhow::bail!("Ollama error {}", res.status());
    }
    let payload: serde_json::Value = res.json()?;
    let mut models = Vec::new();
    if let Some(arr) = payload["models"].as_array() {
        for m in arr {
            if let Some(name) = m["name"].as_str() {
                models.push(name.to_string());
            }
        }
    }
    Ok(models)
}

fn is_lmstudio_available() -> bool {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build();
    if let Ok(client) = client {
        client.get("http://localhost:1234/v1/models").send().is_ok()
    } else {
        false
    }
}

fn fetch_lmstudio_models() -> Result<Vec<String>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?;
    let res = client.get("http://localhost:1234/v1/models").send()?;
    if !res.status().is_success() {
        anyhow::bail!("LM Studio error {}", res.status());
    }
    let payload: serde_json::Value = res.json()?;
    let mut models = Vec::new();
    if let Some(arr) = payload["data"].as_array() {
        for m in arr {
            if let Some(id) = m["id"].as_str() {
                models.push(id.to_string());
            }
        }
    }
    Ok(models)
}

fn fetch_opencode_free_models(api_key: &str) -> Result<Vec<String>> {
    use reqwest::blocking::Client;
    use serde_json::Value;
    use std::time::Duration;

    let client = Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .context("Failed to build HTTP client for OpenCode model fetch")?;

    let mut req = client
        .get("https://opencode.ai/zen/v1/models")
        .header("Accept", "application/json");

    if !api_key.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
    }

    let res = req.send().context("OpenCode /models request failed")?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().unwrap_or_default();
        anyhow::bail!("OpenCode /models error {}: {}", status, body);
    }

    let payload: Value = res
        .json()
        .context("Failed to parse OpenCode /models JSON")?;
    let data = payload
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("OpenCode /models missing data array"))?;

    let mut models: Vec<String> = data
        .iter()
        .filter_map(|m| m.get("id").and_then(|v| v.as_str()))
        .filter(|id| id.to_ascii_lowercase().contains("free"))
        .map(|id| id.to_string())
        .collect();

    models.sort();
    models.dedup();
    Ok(models)
}

fn preferred_opencode_model_index(models: &[String]) -> usize {
    let preferred = [
        "minimax-m2.5-free",
        "gpt-5-nano",
        "big-pickle",
        "qwen-3-coder",
    ];
    for key in preferred {
        if let Some(idx) = models.iter().position(|m| m.eq_ignore_ascii_case(key)) {
            return idx;
        }
    }
    0
}

// ── UX helpers ────────────────────────────────────────────────────

fn section_header(n: u8, title: &str, subtitle: &str) {
    let badge = format!(" Step {} ", n);
    println!(
        "  {} {} {}",
        blue(&format!("┌{}┐", "─".repeat(badge.len()))),
        cyan(&format!("│{}│", badge)),
        dim(title)
    );
    println!("  {}   {}", blue("└"), dim(subtitle));
    println!();
}

fn spin_start(msg: &str) {
    print!("  {}  {} … ", dim("⟳"), msg);
    io::stdout().flush().ok();
}

fn spin_done(msg: &str) {
    println!("{} {}", green("✓"), dim(msg));
}

fn spin_warn(msg: &str) {
    println!("{} {}", yellow("⚠"), dim(msg));
}

fn list_models_simple(models: &[String], default_idx: usize) {
    println!("  {}  {}", dim("№"), dim("Model"));
    println!("  {}", dim(&"─".repeat(50)));
    for (i, m) in models.iter().enumerate() {
        let rec = if i == default_idx {
            format!("  {}", green("← recommended"))
        } else {
            String::new()
        };
        println!("  {}  {}{}", cyan(&format!("{:3}", i + 1)), m, dim(&rec));
    }
    println!();
}

fn summary_row(label: &str, value: &str) {
    println!("     {}  {:<14}  {}", dim("▸"), dim(label), cyan(value));
}

fn provider_docs_url(name: &str) -> &'static str {
    match name {
        "openai" => "https://platform.openai.com/api-keys",
        "anthropic" => "https://console.anthropic.com/keys",
        "openrouter" => "https://openrouter.ai/keys",
        "opencode" => "https://opencode.ai",
        "kilocode" => "https://app.kilo.ai",
        _ => "https://openrouter.ai/keys",
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn prompt_with_default(label: &str, default: &str) -> Result<String> {
    print!("  {} {}: ", label, dim(&format!("[{}]", default)));
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    Ok(if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    })
}

fn prompt_secret(label: &str) -> Result<String> {
    print!("{}: ", bold(label));
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn prompt_choice(label: &str, max: usize, default: usize) -> usize {
    print!(
        "  {} {}: ",
        bold(label),
        dim(&format!("[1-{}, default={}]", max, default))
    );
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();
    let n: usize = input.trim().parse().unwrap_or(default);
    n.clamp(1, max)
}
