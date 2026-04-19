use crate::providers::kilocode::{fetch_kilocode_free_models, preferred_kilocode_model_index};
use crate::providers::openrouter::{
    fetch_openrouter_free_models, format_openrouter_model, preferred_openrouter_model_index,
};
use crate::workspace_templates::*;
use anyhow::{Context, Result};
use dialoguer::{Confirm, Input, Password, Select, theme::ColorfulTheme};
use std::fs;
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// ANSI helpers (used in non-dialoguer output like banners and summaries)
// ─────────────────────────────────────────────────────────────────────────────

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
    ansi!("32", s)
}
fn cyan(s: &str) -> String {
    ansi!("36", s)
}
fn bold_green(s: &str) -> String {
    ansi!("1;32", s)
}
fn bold_cyan(s: &str) -> String {
    ansi!("1;36", s)
}

/// Legacy banner used by main.rs
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
struct PartialConfig {
    provider: Option<ProviderConfig>,
    selected_default_model: Option<String>,
    api_key: Option<String>,
    timezone: Option<String>,
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
    search_provider: Option<String>,
    pushover: Option<Option<(String, String)>>,
    custom_base_url: Option<Option<String>>,
    custom_model: Option<Option<String>>,
    secondary_model: Option<String>,
    secondary_provider_key: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Theme helper
// ─────────────────────────────────────────────────────────────────────────────

fn theme() -> ColorfulTheme {
    ColorfulTheme::default()
}

// ─────────────────────────────────────────────────────────────────────────────
// Main onboarding flow
// ─────────────────────────────────────────────────────────────────────────────

const TOTAL_STEPS: u8 = 9;

const COMMON_TIMEZONES: &[(&str, &str)] = &[
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

    print_banner();

    let mut partial = PartialConfig::default();
    let config_path = dir.join("config.json");
    let mut is_edit = false;

    if config_path.exists() {
        println!("  {} Existing configuration found.\n", bold_cyan("◆"));
        let choice = Select::with_theme(&theme())
            .with_prompt("What would you like to do?")
            .items(&[
                "Edit          — Update specific sections",
                "Reconfigure   — Start fresh (overwrites existing config)",
                "Cancel        — Exit without changes",
            ])
            .default(0)
            .interact()?;

        match choice {
            2 => return Ok(()),
            0 => {
                is_edit = true;
                if let Ok(content) = fs::read_to_string(&config_path) {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                        partial.default_values_from_json(&json);
                    }
                }
            }
            _ => {}
        }
    }

    if is_edit {
        print_edit_mode(&mut partial)?;
    } else {
        print_fresh_setup(&mut partial)?;
    }

    // Write files
    if !dir.exists() {
        fs::create_dir_all(dir).context("Failed to create workspace directory")?;
    }
    let config_json = partial.to_json();
    fs::write(&config_path, serde_json::to_string_pretty(&config_json)?)?;
    let ctx = ProjectContext {
        timezone: partial
            .timezone
            .clone()
            .unwrap_or_else(|| "UTC".to_string()),
        ..ProjectContext::default()
    };
    scaffold_workspace(dir, &ctx)?;

    print_done(&partial, dir);
    Ok(())
}

fn print_fresh_setup(partial: &mut PartialConfig) -> Result<()> {
    println!("  {}  Press Ctrl+C at any time to cancel.\n", dim("·"));

    partial.step_provider(1)?;
    partial.step_secondary_model(2)?;
    partial.step_timezone(3)?;
    partial.step_memory(4)?;
    partial.step_voice(5)?;
    partial.step_channels(6)?;
    partial.step_composio(7)?;
    partial.step_brave(8)?;
    partial.step_pushover(9)?;
    Ok(())
}

fn print_edit_mode(partial: &mut PartialConfig) -> Result<()> {
    loop {
        println!();
        let items = &[
            "AI Provider & Model  — Change provider, model, or API key",
            "Background Tasks     — Secondary cheaper model for background tasks",
            "Timezone             — Your local timezone",
            "Memory & Embeddings  — Switch memory backend or embeddings",
            "Voice                — Groq Whisper transcription",
            "Channels             — Telegram, WhatsApp, and other messaging",
            "Composio             — External app integrations",
            "Web Search           — Gemini CLI, Brave, or DuckDuckGo",
            "Pushover             — Push notification alerts",
            "─────────────────────────────────────────────────────",
            "Save & Exit          — Write config and quit",
            "Discard & Exit       — Exit without saving",
        ];

        let choice = Select::with_theme(&theme())
            .with_prompt("Which section would you like to edit?")
            .items(items)
            .default(10)
            .interact()?;

        match choice {
            0 => partial.step_provider(1)?,
            1 => partial.step_secondary_model(2)?,
            2 => partial.step_timezone(3)?,
            3 => partial.step_memory(4)?,
            4 => partial.step_voice(5)?,
            5 => partial.step_channels(6)?,
            6 => partial.step_composio(7)?,
            7 => partial.step_brave(8)?,
            8 => partial.step_pushover(9)?,
            9 => {} // separator — do nothing
            10 => break,
            _ => return Ok(()),
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Step implementations
// ─────────────────────────────────────────────────────────────────────────────

impl PartialConfig {
    fn default_values_from_json(&mut self, json: &serde_json::Value) {
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

    fn step_provider(&mut self, step: u8) -> Result<()> {
        step_header(
            step,
            TOTAL_STEPS,
            "AI Provider",
            "Which AI service powers your agent?",
        );

        // (id, display_label, source_tag, static_default_model)
        let providers: &[(&str, &str, &str, &str)] = &[
            ("gemini", "Gemini", "Google", "gemini-2.0-flash"),
            ("openai", "GPT-4o", "OpenAI", "gpt-4o"),
            ("anthropic", "Claude", "Anthropic", "claude-sonnet-4-5"),
            (
                "openrouter",
                "OpenRouter",
                "OpenRouter",
                "deepseek/deepseek-chat-v3-0324:free",
            ),
            ("opencode", "OpenCode", "OpenCode", "minimax-m2.5-free"),
            (
                "kilocode",
                "Kilocode",
                "Kilo.ai",
                "minimax/minimax-m2.1:free",
            ),
            ("ollama", "Ollama", "Local", "llama3.2"),
            ("lmstudio", "LM Studio", "Local", "local-model"),
            ("openai-compatible", "Custom / Compatible", "Custom", ""),
        ];

        let default_idx = self
            .provider
            .as_ref()
            .and_then(|p| {
                providers
                    .iter()
                    .position(|(name, ..)| *name == p.name.as_str())
            })
            .unwrap_or(0);

        let items: Vec<String> = providers
            .iter()
            .map(|(_, label, source, model)| {
                let tag = format!("[{}]", source);
                if model.is_empty() {
                    format!("{:<24} {:<14}", label, tag)
                } else {
                    format!("{:<24} {:<14} {}", label, tag, model)
                }
            })
            .collect();

        let choice = Select::with_theme(&theme())
            .with_prompt("Select AI provider")
            .items(&items)
            .default(default_idx)
            .interact()?;

        let (p_name, _, _, p_model) = providers[choice];

        let mut pc = ProviderConfig {
            name: p_name.to_string(),
            default_model: p_model.to_string(),
            base_url: None,
            model: None,
        };

        match p_name {
            "gemini" => {
                pc.base_url = None;
                println!();
                let auth_choice = Select::with_theme(&theme())
                    .with_prompt("Authentication method")
                    .items(&[
                        "API Key    — Paste your Gemini API key",
                        "CLI OAuth  — Use existing Gemini CLI login (no key needed)",
                    ])
                    .default(0)
                    .interact()?;

                if auth_choice == 1 {
                    self.api_key = Some("cli_oauth".to_string());
                    step_done("Gemini · CLI OAuth");
                } else {
                    let key: String = Password::with_theme(&theme())
                        .with_prompt("Gemini API key")
                        .with_confirmation("Confirm API key", "Keys don't match")
                        .allow_empty_password(true)
                        .interact()?;
                    self.api_key = Some(key.clone());

                    // Try to fetch available models
                    println!("  {} Fetching available models…", dim("⟳"));
                    match fetch_gemini_models(&key) {
                        Ok(models) if !models.is_empty() => {
                            println!("  {} {} models available\n", bold_green("✓"), models.len());
                            let d = models
                                .iter()
                                .position(|m| m.contains("2.0-flash"))
                                .unwrap_or(0);
                            let mc = Select::with_theme(&theme())
                                .with_prompt("Select Gemini model")
                                .items(&models)
                                .default(d)
                                .interact()?;
                            pc.default_model = models[mc].clone();
                        }
                        Ok(_) | Err(_) => {
                            eprintln!("  {} Could not fetch models, using defaults.", dim("·"));
                        }
                    }
                    step_done(&format!("Gemini · {}", pc.default_model));
                }
            }

            "openai" => {
                let key: String = Password::with_theme(&theme())
                    .with_prompt("OpenAI API key")
                    .with_confirmation("Confirm API key", "Keys don't match")
                    .allow_empty_password(true)
                    .interact()?;
                self.api_key = Some(key.clone());

                println!("  {} Fetching available models…", dim("⟳"));
                match fetch_openai_models(&key) {
                    Ok(models) if !models.is_empty() => {
                        println!("  {} {} models available\n", bold_green("✓"), models.len());
                        let d = models
                            .iter()
                            .position(|m| m.contains("gpt-4o") && !m.contains("mini"))
                            .unwrap_or(0);
                        let mc = Select::with_theme(&theme())
                            .with_prompt("Select OpenAI model")
                            .items(&models)
                            .default(d)
                            .interact()?;
                        pc.default_model = models[mc].clone();
                    }
                    Ok(_) | Err(_) => {
                        eprintln!("  {} Could not fetch models, using defaults.", dim("·"));
                    }
                }
                step_done(&format!("OpenAI · {}", pc.default_model));
            }

            "anthropic" => {
                let key: String = Password::with_theme(&theme())
                    .with_prompt("Anthropic API key")
                    .with_confirmation("Confirm API key", "Keys don't match")
                    .allow_empty_password(true)
                    .interact()?;
                self.api_key = Some(key.clone());

                println!("  {} Fetching available models…", dim("⟳"));
                match fetch_anthropic_models(&key) {
                    Ok(models) if !models.is_empty() => {
                        println!("  {} {} models available\n", bold_green("✓"), models.len());
                        let d = models
                            .iter()
                            .position(|m| m.contains("sonnet"))
                            .unwrap_or(0);
                        let mc = Select::with_theme(&theme())
                            .with_prompt("Select Anthropic model")
                            .items(&models)
                            .default(d)
                            .interact()?;
                        pc.default_model = models[mc].clone();
                    }
                    Ok(_) | Err(_) => {
                        eprintln!("  {} Could not fetch models, using defaults.", dim("·"));
                    }
                }
                step_done(&format!("Anthropic · {}", pc.default_model));
            }

            "openrouter" => {
                pc.base_url = Some("https://openrouter.ai/api/v1".to_string());
                let key: String = Password::with_theme(&theme())
                    .with_prompt("OpenRouter API key  (openrouter.ai/keys)")
                    .with_confirmation("Confirm API key", "Keys don't match")
                    .allow_empty_password(true)
                    .interact()?;
                self.api_key = Some(key.clone());

                println!("  {} Fetching free models…", dim("⟳"));
                match fetch_openrouter_free_models(&key) {
                    Ok(free_models) if !free_models.is_empty() => {
                        println!(
                            "  {} {} free models available\n",
                            bold_green("✓"),
                            free_models.len()
                        );
                        let d = preferred_openrouter_model_index(&free_models);
                        let display: Vec<String> = free_models
                            .iter()
                            .map(|m| format_openrouter_model(m))
                            .collect();
                        let mc = Select::with_theme(&theme())
                            .with_prompt("Select model")
                            .items(&display)
                            .default(d)
                            .interact()?;
                        pc.default_model = free_models[mc].id.clone();
                    }
                    Ok(_) | Err(_) => {
                        eprintln!("  {} Could not fetch models, using defaults.", dim("·"));
                    }
                }
                step_done(&format!("OpenRouter · {}", pc.default_model));
            }

            "opencode" => {
                pc.base_url = Some("https://opencode.ai/zen/v1".to_string());
                let key: String = Password::with_theme(&theme())
                    .with_prompt("OpenCode API key  (opencode.ai)")
                    .with_confirmation("Confirm API key", "Keys don't match")
                    .allow_empty_password(true)
                    .interact()?;
                self.api_key = Some(key.clone());

                // Try OpenAI-compatible /models
                println!("  {} Fetching available models…", dim("⟳"));
                match fetch_openai_compat_models("https://opencode.ai/zen/v1", &key) {
                    Ok(models) if !models.is_empty() => {
                        println!("  {} {} models available\n", bold_green("✓"), models.len());
                        let d = models.iter().position(|m| m.contains("m2.5")).unwrap_or(0);
                        let mc = Select::with_theme(&theme())
                            .with_prompt("Select model")
                            .items(&models)
                            .default(d)
                            .interact()?;
                        pc.default_model = models[mc].clone();
                    }
                    Ok(_) | Err(_) => {
                        eprintln!("  {} Could not fetch models, using static list.", dim("·"));
                        let static_models = &[
                            "minimax-m2.5-free",
                            "minimax-m2.5-pro",
                            "qwen-coder-plus-latest",
                        ];
                        let mc = Select::with_theme(&theme())
                            .with_prompt("Select model")
                            .items(static_models)
                            .default(0)
                            .interact()?;
                        pc.default_model = static_models[mc].to_string();
                    }
                }
                step_done(&format!("OpenCode · {}", pc.default_model));
            }

            "kilocode" => {
                pc.base_url = Some("https://api.kilo.ai/api/gateway".to_string());
                let key: String = Password::with_theme(&theme())
                    .with_prompt("Kilocode API key  (app.kilo.ai)")
                    .with_confirmation("Confirm API key", "Keys don't match")
                    .allow_empty_password(true)
                    .interact()?;
                self.api_key = Some(key.clone());

                println!("  {} Fetching free models…", dim("⟳"));
                match fetch_kilocode_free_models(&key) {
                    Ok(free_models) if !free_models.is_empty() => {
                        println!(
                            "  {} {} free models available\n",
                            bold_green("✓"),
                            free_models.len()
                        );
                        let d = preferred_kilocode_model_index(&free_models);
                        let mc = Select::with_theme(&theme())
                            .with_prompt("Select model")
                            .items(&free_models)
                            .default(d)
                            .interact()?;
                        pc.default_model = free_models[mc].clone();
                        self.kilocode_fallback_models = Some(free_models);
                    }
                    Ok(_) | Err(_) => {
                        eprintln!("  {} Could not fetch models, using defaults.", dim("·"));
                    }
                }
                step_done(&format!("Kilocode · {}", pc.default_model));
            }

            "ollama" => {
                pc.base_url = Some("http://localhost:11434/v1".to_string());
                self.api_key = Some(String::new());

                println!("  {} Fetching local models from Ollama…", dim("⟳"));
                match fetch_ollama_models() {
                    Ok(models) if !models.is_empty() => {
                        println!("  {} {} models installed\n", bold_green("✓"), models.len());
                        let d = models.iter().position(|m| m.contains("qwen")).unwrap_or(0);
                        let mc = Select::with_theme(&theme())
                            .with_prompt("Select model")
                            .items(&models)
                            .default(d)
                            .interact()?;
                        pc.default_model = models[mc].clone();
                    }
                    Ok(_) => {
                        eprintln!(
                            "  {} No models found. Run `ollama pull llama3.2` first.",
                            dim("·")
                        );
                    }
                    Err(_) => {
                        eprintln!("  {} Ollama not reachable at localhost:11434.", dim("·"));
                    }
                }
                step_done(&format!("Ollama · {}", pc.default_model));
            }

            "lmstudio" => {
                pc.base_url = Some("http://localhost:1234/v1".to_string());
                self.api_key = Some(String::new());

                println!("  {} Fetching local models from LM Studio…", dim("⟳"));
                match fetch_ollama_compat_models("http://localhost:1234/v1") {
                    Ok(models) if !models.is_empty() => {
                        println!("  {} {} models loaded\n", bold_green("✓"), models.len());
                        let mc = Select::with_theme(&theme())
                            .with_prompt("Select model")
                            .items(&models)
                            .default(0)
                            .interact()?;
                        pc.default_model = models[mc].clone();
                    }
                    Ok(_) | Err(_) => {
                        let model: String = Input::with_theme(&theme())
                            .with_prompt("Model name (as shown in LM Studio)")
                            .default("local-model".to_string())
                            .interact_text()?;
                        pc.default_model = model;
                    }
                }
                step_done(&format!("LM Studio · {}", pc.default_model));
            }

            "openai-compatible" => {
                let url: String = Input::with_theme(&theme())
                    .with_prompt("Base URL")
                    .default("http://localhost:8080/v1".to_string())
                    .interact_text()?;

                let key: String = Password::with_theme(&theme())
                    .with_prompt("API key (leave blank if not required)")
                    .allow_empty_password(true)
                    .interact()?;

                // Try to list models from the custom endpoint
                println!("  {} Fetching models from {}…", dim("⟳"), url);
                let model = match fetch_openai_compat_models(&url, &key) {
                    Ok(models) if !models.is_empty() => {
                        println!("  {} {} models found\n", bold_green("✓"), models.len());
                        let mc = Select::with_theme(&theme())
                            .with_prompt("Select model")
                            .items(&models)
                            .default(0)
                            .interact()?;
                        models[mc].clone()
                    }
                    _ => {
                        eprintln!("  {} Could not auto-detect models.", dim("·"));
                        Input::with_theme(&theme())
                            .with_prompt("Model name")
                            .default("gpt-4o".to_string())
                            .interact_text()?
                    }
                };

                pc.base_url = Some(url);
                pc.default_model = model.clone();
                pc.model = Some(model.clone());
                self.api_key = Some(key);
                self.custom_base_url = Some(pc.base_url.clone());
                self.custom_model = Some(pc.model.clone());
                step_done(&format!("Custom · {}", model));
            }

            _ => {}
        }

        self.selected_default_model = Some(pc.default_model.clone());
        self.provider = Some(pc);
        Ok(())
    }

    fn step_memory(&mut self, step: u8) -> Result<()> {
        step_header(
            step,
            TOTAL_STEPS,
            "Memory",
            "How should your agent remember things?",
        );

        let backends = &[
            "SQLite    — Fast local database, supports semantic search",
            "Markdown  — Human-readable files in your workspace",
            "None      — Ephemeral, no memory between sessions",
        ];
        let default = match self.memory_backend.as_deref() {
            Some("markdown") => 1,
            Some("none") => 2,
            _ => 0,
        };
        let choice = Select::with_theme(&theme())
            .with_prompt("Memory backend")
            .items(backends)
            .default(default)
            .interact()?;

        let backend = match choice {
            1 => "markdown",
            2 => "none",
            _ => "sqlite",
        };
        self.memory_backend = Some(backend.to_string());

        if backend == "sqlite" {
            println!();
            let embed = Confirm::with_theme(&theme())
                .with_prompt("Enable vector embeddings for semantic recall?")
                .default(false)
                .interact()?;
            if embed {
                self.embed_provider = Some(Some("huggingface".to_string()));
                self.embed_model = Some(Some("Qwen/Qwen3-Embedding-0.6B".to_string()));
                self.embed_key = Some(Some(String::new()));
                step_done("SQLite · embeddings enabled");
            } else {
                step_done("SQLite · keyword search");
            }
        } else {
            step_done(&capitalize(backend));
        }
        Ok(())
    }

    fn step_voice(&mut self, step: u8) -> Result<()> {
        step_header(
            step,
            TOTAL_STEPS,
            "Voice",
            "Groq Whisper speech-to-text (optional)",
        );
        let current = self
            .groq_key
            .as_deref()
            .map(|k| !k.is_empty())
            .unwrap_or(false);

        let enable = Confirm::with_theme(&theme())
            .with_prompt("Enable voice transcription via Groq Whisper?")
            .default(current)
            .interact()?;

        if enable {
            let key: String = Password::with_theme(&theme())
                .with_prompt("Groq API key  (console.groq.com/keys)")
                .with_confirmation("Confirm API key", "Keys don't match")
                .allow_empty_password(true)
                .interact()?;
            self.groq_key = Some(key);
            step_done("Voice · Groq Whisper");
        } else {
            self.groq_key = None;
            step_skip("Voice");
        }
        Ok(())
    }

    fn step_secondary_model(&mut self, step: u8) -> Result<()> {
        step_header(
            step,
            TOTAL_STEPS,
            "Background Tasks Model",
            "Optional: Choose a cheaper model for background tasks (e.g., cron, summarization)",
        );

        let choice = Confirm::with_theme(&theme())
            .with_prompt("Configure a secondary model for background tasks?")
            .default(false)
            .interact()?;

        if !choice {
            step_skip("Using default model for all tasks");
            return Ok(());
        }

        let model: String = Input::with_theme(&theme())
            .with_prompt(
                "Enter the secondary model ID (e.g., nvidia/nemotron-3-super-120b-a12b:free)",
            )
            .interact_text()?;

        self.secondary_model = Some(model.trim().to_string());

        let use_openrouter = Confirm::with_theme(&theme())
            .with_prompt("Is this an OpenRouter model?")
            .default(self.secondary_model.as_ref().unwrap().contains("/"))
            .interact()?;

        if use_openrouter {
            let key: String = Input::with_theme(&theme())
                .with_prompt("OpenRouter API key (leave blank to skip if already configured as default provider)")
                .allow_empty(true)
                .interact_text()?;
            let key = key.trim().to_string();
            if !key.is_empty() {
                self.secondary_provider_key = Some(key);
            }
        }

        step_done(&format!(
            "Background tasks model set to {}",
            self.secondary_model.as_ref().unwrap()
        ));
        Ok(())
    }

    fn step_timezone(&mut self, step: u8) -> Result<()> {
        step_header(
            step,
            TOTAL_STEPS,
            "Timezone",
            "So the agent knows when it's late and when to reach you",
        );

        let current_tz = self.timezone.as_deref().unwrap_or("UTC");
        let default_idx = COMMON_TIMEZONES
            .iter()
            .position(|(tz, _)| *tz == current_tz)
            .unwrap_or(0);

        let items: Vec<String> = COMMON_TIMEZONES
            .iter()
            .map(|(tz, label)| format!("{:<24} {}", label, dim(tz)))
            .collect();

        let choice = Select::with_theme(&theme())
            .with_prompt("Your timezone")
            .items(&items)
            .default(default_idx)
            .interact()?;

        let tz = COMMON_TIMEZONES[choice].0.to_string();
        step_done(&format!("Timezone · {}", tz));
        self.timezone = Some(tz);
        Ok(())
    }

    fn step_channels(&mut self, step: u8) -> Result<()> {
        step_header(
            step,
            TOTAL_STEPS,
            "Channels",
            "How will you talk to your agent?",
        );

        let has_tg = self.telegram.as_ref().and_then(|t| t.as_ref()).is_some();
        let has_wa = self
            .whatsapp_native
            .as_ref()
            .and_then(|w| w.as_ref())
            .is_some();

        let default_idx = match (has_tg, has_wa) {
            (true, false) => 1,
            (false, true) => 2,
            (true, true) => 3,
            _ => 0,
        };

        let choice = Select::with_theme(&theme())
            .with_prompt("Which channel will you use to reach the agent?")
            .items(&[
                "None      — CLI only, no messaging integration",
                "Telegram  — Talk to the agent via a Telegram bot",
                "WhatsApp  — Talk to the agent via WhatsApp (local bridge)",
                "Both      — Telegram + WhatsApp",
            ])
            .default(default_idx)
            .interact()?;

        // ── Telegram ──────────────────────────────────────────────────
        if choice == 1 || choice == 3 {
            println!();
            println!(
                "  {}  Create your bot at {}",
                dim("tip"),
                cyan("t.me/BotFather")
            );
            println!();
            let token: String = Password::with_theme(&theme())
                .with_prompt("Bot token")
                .allow_empty_password(true)
                .interact()?;
            let user: String = Input::with_theme(&theme())
                .with_prompt("Your Telegram username (e.g. @you)")
                .default("@me".to_string())
                .interact_text()?;
            self.telegram = Some(Some((token, user)));
        } else {
            self.telegram = Some(None);
        }

        // ── WhatsApp ──────────────────────────────────────────────────
        if choice == 2 || choice == 3 {
            println!();
            let phone: String = Input::with_theme(&theme())
                .with_prompt("Your WhatsApp phone number (e.g. +1234567890)")
                .default("+1".to_string())
                .interact_text()?;
            self.whatsapp_native = Some(Some(("http://localhost:18790".to_string(), phone)));
        } else {
            self.whatsapp_native = Some(None);
        }

        let summary = match choice {
            1 => format!(
                "Telegram · {}",
                self.telegram
                    .as_ref()
                    .and_then(|t| t.as_ref())
                    .map(|(_, u)| u.as_str())
                    .unwrap_or("")
            ),
            2 => format!(
                "WhatsApp · {}",
                self.whatsapp_native
                    .as_ref()
                    .and_then(|w| w.as_ref())
                    .map(|(_, p)| p.as_str())
                    .unwrap_or("")
            ),
            3 => "Telegram + WhatsApp".to_string(),
            _ => "CLI only".to_string(),
        };
        step_done(&summary);
        Ok(())
    }

    fn step_composio(&mut self, step: u8) -> Result<()> {
        step_header(
            step,
            TOTAL_STEPS,
            "Composio",
            "Connect external apps like GitHub, Gmail, Notion",
        );
        let has = self.composio_enabled.unwrap_or(false);

        let enable = Confirm::with_theme(&theme())
            .with_prompt("Enable Composio?")
            .default(has)
            .interact()?;

        if enable {
            let key: String = Password::with_theme(&theme())
                .with_prompt("Composio API key  (app.composio.dev)")
                .with_confirmation("Confirm API key", "Keys don't match")
                .allow_empty_password(true)
                .interact()?;
            let entity: String = Input::with_theme(&theme())
                .with_prompt("Entity ID")
                .default("default".to_string())
                .interact_text()?;
            self.composio_enabled = Some(true);
            self.composio_api_key = Some(Some(key));
            self.composio_entity_id = Some(entity);
            step_done("Composio · enabled");
        } else {
            self.composio_enabled = Some(false);
            step_skip("Composio");
        }
        Ok(())
    }

    fn step_brave(&mut self, step: u8) -> Result<()> {
        step_header(
            step,
            TOTAL_STEPS,
            "Web Search",
            "How should your agent search the web?",
        );

        let items = &[
            "Gemini CLI  — Free, uses installed Gemini CLI with built-in Google Search (recommended)",
            "Brave API   — Brave Search API key required (brave.com/search/api)",
            "DuckDuckGo  — Free but rate-limited, no API key needed",
            "None        — Skip web search setup",
        ];

        let has_brave = self
            .brave_api_key
            .as_ref()
            .and_then(|k| k.as_ref())
            .is_some();
        let has_gemini = which::which("gemini").is_ok();

        let default = if has_gemini {
            0
        } else if has_brave {
            1
        } else {
            0
        };

        let choice = Select::with_theme(&theme())
            .with_prompt("Select web search provider")
            .items(items)
            .default(default)
            .interact()?;

        match choice {
            0 => {
                self.search_provider = Some("gemini_cli".to_string());
                if has_gemini {
                    self.brave_api_key = Some(None);
                    step_done("Web Search · Gemini CLI");
                } else {
                    println!("\n  {} Gemini CLI not found on PATH.", bold("!"));
                    println!(
                        "  Install it first: {}\n",
                        cyan("npm install -g @google/gemini-cli")
                    );
                    let proceed = Confirm::with_theme(&theme())
                        .with_prompt("Use Gemini CLI anyway? (install it later)")
                        .default(true)
                        .interact()?;
                    if proceed {
                        self.brave_api_key = Some(None);
                        step_done("Web Search · Gemini CLI (install pending)");
                    } else {
                        self.brave_api_key = Some(None);
                        step_skip("Web Search");
                    }
                }
            }
            1 => {
                self.search_provider = Some("brave".to_string());
                let key: String = Password::with_theme(&theme())
                    .with_prompt("Brave API key  (brave.com/search/api)")
                    .with_confirmation("Confirm API key", "Keys don't match")
                    .allow_empty_password(true)
                    .interact()?;
                self.brave_api_key = Some(Some(key));
                step_done("Web Search · Brave");
            }
            2 => {
                self.search_provider = Some("duckduckgo".to_string());
                self.brave_api_key = Some(None);
                step_done("Web Search · DuckDuckGo");
            }
            _ => {
                self.search_provider = Some("duckduckgo".to_string());
                self.brave_api_key = Some(None);
                step_skip("Web Search");
            }
        }
        Ok(())
    }

    fn step_pushover(&mut self, step: u8) -> Result<()> {
        step_header(
            step,
            TOTAL_STEPS,
            "Pushover",
            "Desktop & mobile push notifications (optional)",
        );
        let has = self.pushover.as_ref().and_then(|p| p.as_ref()).is_some();

        let enable = Confirm::with_theme(&theme())
            .with_prompt("Enable Pushover notifications?")
            .default(has)
            .interact()?;

        if enable {
            let token: String = Password::with_theme(&theme())
                .with_prompt("App token  (pushover.net/apps)")
                .allow_empty_password(true)
                .interact()?;
            let user: String = Password::with_theme(&theme())
                .with_prompt("User key  (pushover.net)")
                .allow_empty_password(true)
                .interact()?;
            self.pushover = Some(Some((token, user)));
            step_done("Pushover · enabled");
        } else {
            self.pushover = Some(None);
            step_skip("Pushover");
        }
        Ok(())
    }

    fn to_json(&self) -> serde_json::Value {
        use serde_json::json;

        let p_name = self
            .provider
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "gemini".to_string());
        let mut providers = json!({
            &p_name: { "api_key": self.api_key.as_ref().cloned().unwrap_or_default() }
        });
        if let Some(Some(m)) = &self.custom_model {
            providers[&p_name]["model"] = json!(m);
        }
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

        if let Some(sec_model) = &self.secondary_model {
            if sec_model.contains("/") {
                // Assuming openrouter if it has a slash (as in nvidia/nemotron...)
                if "openrouter" != p_name.as_str() {
                    let key = self
                        .secondary_provider_key
                        .as_ref()
                        .cloned()
                        .unwrap_or_default();
                    providers["openrouter"] = json!({
                        "api_key": key,
                        "base_url": "https://openrouter.ai/api/v1",
                        "model": sec_model
                    });
                }
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

        let mut config = json!({
            "default_provider": p_name,
            "default_model": self.selected_default_model.as_ref().cloned().unwrap_or_else(|| "gemini-2.0-flash".to_string()),
            "timezone": self.timezone.as_ref().cloned().unwrap_or_else(|| "UTC".to_string()),
            "models": { "providers": providers },
            "channels": { "telegram": telegram_vec, "whatsapp_native": whatsapp_vec },
            "memory": { "backend": self.memory_backend.as_ref().cloned().unwrap_or_else(|| "sqlite".to_string()) },
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

        if let Some(sec_model) = &self.secondary_model {
            config["task_models"] = json!({
                "cron": sec_model,
                "event": sec_model,
                "greeting": sec_model,
                "heartbeat": sec_model,
                "subagent": sec_model,
                "summarize": sec_model
            });
        }

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
// Dynamic model fetching
// ─────────────────────────────────────────────────────────────────────────────

fn fetch_ollama_models() -> Result<Vec<String>> {
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

/// Fetch models from any OpenAI-compatible /models endpoint.
fn fetch_openai_compat_models(base_url: &str, api_key: &str) -> Result<Vec<String>> {
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

/// Fetch LM Studio local models via their OpenAI-compat /models endpoint.
fn fetch_ollama_compat_models(base_url: &str) -> Result<Vec<String>> {
    fetch_openai_compat_models(base_url, "")
}

/// Fetch available Gemini models for a given API key.
fn fetch_gemini_models(api_key: &str) -> Result<Vec<String>> {
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

    // Prefer newer models first
    models.sort_by(|a, b| b.cmp(a));
    Ok(models)
}

/// Fetch available OpenAI chat/reasoning models.
fn fetch_openai_models(api_key: &str) -> Result<Vec<String>> {
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
            // Keep only chat/reasoning models
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

    // Newest first
    models.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(models.into_iter().map(|(id, _)| id).collect())
}

/// Fetch available Anthropic Claude models.
fn fetch_anthropic_models(api_key: &str) -> Result<Vec<String>> {
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

    // Newest first (ISO timestamp string sort works)
    models.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(models.into_iter().map(|(id, _)| id).collect())
}

// ─────────────────────────────────────────────────────────────────────────────
// UI helpers
// ─────────────────────────────────────────────────────────────────────────────

fn print_banner() {
    println!();
    println!("  ┌─────────────────────────────────────────────────────┐");
    println!("  │                                                     │");
    println!(
        "  │   {}  {}                              │",
        bold_cyan("🐾"),
        bold("OpenPaw  ·  Agent Setup Wizard")
    );
    println!(
        "  │   {}                      │",
        dim("Powered by Rust · open-source · runs anywhere")
    );
    println!("  │                                                     │");
    println!("  └─────────────────────────────────────────────────────┘");
    println!();
}

fn step_header(step: u8, total: u8, title: &str, subtitle: &str) {
    println!();
    println!(
        "  {} {}  {}  {}",
        dim(&format!("── {}/{}", step, total)),
        bold_cyan(&format!("[{}]", title)),
        dim("─────────────────────────────────────────────"),
        dim(subtitle)
    );
    println!();
}

fn step_done(detail: &str) {
    println!();
    println!("  {} {}", bold_green("✓"), green(detail));
    println!();
}

fn step_skip(label: &str) {
    println!();
    println!("  {} {} {}", dim("·"), dim(label), dim("skipped"));
    println!();
}

fn print_done(partial: &PartialConfig, dir: &Path) {
    let workspace = dir
        .canonicalize()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| dir.display().to_string());

    println!();
    println!("  ┌─────────────────────────────────────────────────────┐");
    println!(
        "  │  {}  {}                        │",
        bold_green("✓"),
        bold("Setup complete! OpenPaw is ready.")
    );
    println!("  └─────────────────────────────────────────────────────┘");
    println!();
    println!("  {}  {}", bold_cyan("◆"), bold("Configuration summary"));
    println!();

    let prov = partial
        .provider
        .as_ref()
        .map(|p| {
            let model = partial
                .selected_default_model
                .as_deref()
                .unwrap_or(&p.default_model);
            format!("{} · {}", capitalize(&p.name), model)
        })
        .unwrap_or_else(|| "not set".to_string());
    summary_row("Provider", &prov);

    let tz = partial.timezone.as_deref().unwrap_or("UTC");
    summary_row("Timezone", tz);

    let mem = partial
        .memory_backend
        .as_deref()
        .map(|b| {
            if b == "sqlite" && partial.embed_model.is_some() {
                "SQLite + embeddings"
            } else {
                b
            }
        })
        .unwrap_or("sqlite");
    summary_row("Memory", mem);

    let voice = if partial
        .groq_key
        .as_deref()
        .map(|k| !k.is_empty())
        .unwrap_or(false)
    {
        "Groq Whisper"
    } else {
        "disabled"
    };
    summary_row("Voice", voice);

    let channels = {
        let tg = partial
            .telegram
            .as_ref()
            .and_then(|t| t.as_ref())
            .map(|(_, u)| u.as_str());
        let wa = partial
            .whatsapp_native
            .as_ref()
            .and_then(|w| w.as_ref())
            .map(|(_, p)| p.as_str());
        match (tg, wa) {
            (Some(t), Some(w)) => format!("Telegram ({}) + WhatsApp ({})", t, w),
            (Some(t), None) => format!("Telegram ({})", t),
            (None, Some(w)) => format!("WhatsApp ({})", w),
            _ => "CLI only".to_string(),
        }
    };
    summary_row("Channels", &channels);

    let search = if partial
        .brave_api_key
        .as_ref()
        .and_then(|k| k.as_ref())
        .is_some()
    {
        "Brave"
    } else {
        "DuckDuckGo (built-in)"
    };
    summary_row("Web search", search);

    let push = if partial.pushover.as_ref().and_then(|p| p.as_ref()).is_some() {
        "enabled"
    } else {
        "disabled"
    };
    summary_row("Pushover", push);

    println!();
    summary_row("Workspace", &workspace);

    println!();
    println!(
        "  {}  {}  {}",
        dim("──"),
        bold("Next steps"),
        dim("──────────────────────────────────────────────")
    );
    println!();
    println!(
        "  {}  Start the agent    {}",
        dim("❯"),
        bold_cyan("openpaw agent")
    );
    println!("  {}  Reconfigure        {}", dim("·"), dim("openpaw init"));
    println!(
        "  {}  Daemon mode        {}",
        dim("·"),
        dim("openpaw daemon")
    );
    println!();
}

fn summary_row(label: &str, value: &str) {
    let colored = if value == "disabled" || value == "not set" {
        dim(value)
    } else {
        cyan(value)
    };
    println!("  {}  {:<14}  {}", dim("▸"), dim(label), colored);
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}
