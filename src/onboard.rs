use crate::providers::kilocode::{
    DEFAULT_FREE_MODELS, fetch_kilocode_free_models, preferred_kilocode_model_index,
};
use crate::providers::ollama::OllamaProvider;
use crate::providers::openrouter::{
    fetch_openrouter_free_models, format_openrouter_model, preferred_openrouter_model_index,
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
fn red(s: &str) -> String {
    ansi!("1;31", s)
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

    // ── Already initialized? ─────────────────────────────────────────
    if dir.join("config.json").exists() {
        print!(
            "  {}  Config already exists. Overwrite? {}: ",
            warn_icon(),
            dim("(y/N)")
        );
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("  {}  Keeping your existing config. Bye! 👋", ok());
            return Ok(());
        }
        println!();
    }

    // ══════════════════════════════════════════════════════════════════
    // STEP 1 — AI Provider
    // ══════════════════════════════════════════════════════════════════
    section_header(
        1,
        "AI Provider",
        "Which AI service should power your agent?",
    );

    let ollama_available = OllamaProvider::new(None).is_available();
    let lmstudio_available = is_lmstudio_available();

    if ollama_available {
        println!(
            "  {}  {} Ollama detected running locally! Recommended for 100% privacy.",
            ok(),
            green("Local AI:")
        );
    }
    if lmstudio_available {
        println!(
            "  {}  {} LM Studio detected running locally!",
            ok(),
            green("Local AI:")
        );
    }

    let providers = vec![
        ProviderConfig {
            name: "gemini".to_string(),
            default_model: "gemini-2.0-flash".to_string(),
            base_url: None,
        },
        ProviderConfig {
            name: "openai".to_string(),
            default_model: "gpt-4o".to_string(),
            base_url: None,
        },
        ProviderConfig {
            name: "anthropic".to_string(),
            default_model: "claude-3-5-sonnet-latest".to_string(),
            base_url: None,
        },
        ProviderConfig {
            name: "openrouter".to_string(),
            default_model: "deepseek/deepseek-chat-v3-0324:free".to_string(),
            base_url: Some("https://openrouter.ai/api/v1".to_string()),
        },
        ProviderConfig {
            name: "opencode".to_string(),
            default_model: "minimax-m2.5-free".to_string(),
            base_url: Some("https://opencode.ai/zen/v1".to_string()),
        },
        ProviderConfig {
            name: "kilocode".to_string(),
            default_model: "minimax/minimax-m2.1:free".to_string(),
            base_url: Some("https://api.kilo.ai/api/gateway".to_string()),
        },
        ProviderConfig {
            name: "ollama".to_string(),
            default_model: "llama3.2".to_string(),
            base_url: Some("http://localhost:11434/v1".to_string()),
        },
        ProviderConfig {
            name: "lmstudio".to_string(),
            default_model: "local-model".to_string(),
            base_url: Some("http://localhost:1234/v1".to_string()),
        },
    ];

    // Provider table
    println!();
    println!(
        "  {}  {:<14}  {:<12}  {}",
        dim("№"),
        dim("Provider"),
        dim("Key"),
        dim("Notes")
    );
    println!("  {}", dim(&"─".repeat(62)));
    let badges: &[(&str, &str, &str)] = &[
        (
            "gemini",
            "Required",
            "Free tier — 15 req/min, 1M tokens/day",
        ),
        ("openai", "Required", "GPT-4o, o1, o3 — pay-per-use"),
        ("anthropic", "Required", "Claude 3.5 / 3.7 — pay-per-use"),
        (
            "openrouter",
            "Required",
            "200+ models, free tier available → live fetch",
        ),
        (
            "opencode",
            "Optional",
            "OpenCode Zen — free models auto-detected",
        ),
        (
            "kilocode",
            "Required",
            "Kilo.ai Gateway — free + 200+ paid models",
        ),
        ("ollama", "None", "100% local, no key needed"),
        ("lmstudio", "None", "100% local, no key needed"),
    ];
    for (i, p) in providers.iter().enumerate() {
        let (_, key_label, note) = badges[i];
        let key_col = if key_label == "None" {
            green(key_label)
        } else if key_label == "Optional" {
            yellow(key_label)
        } else {
            dim(key_label)
        };
        println!(
            "  {}  {:<14}  {:<22}  {}",
            cyan(&format!("{}", i + 1)),
            bold(&p.name),
            key_col,
            dim(note)
        );
    }
    println!();
    let provider_idx = prompt_choice("  Select provider", providers.len(), 1) - 1;
    let provider = &providers[provider_idx];
    println!();

    // ── API Key ──────────────────────────────────────────────────────
    let api_key = if provider.name == "ollama" || provider.name == "lmstudio" {
        println!(
            "  {}  {} runs locally — {}",
            ok(),
            capitalize(&provider.name),
            dim("no API key needed")
        );
        String::new()
    } else if provider.name == "gemini" {
        println!(
            "  {}  Get your free key at {}",
            info(),
            cyan("https://aistudio.google.com/apikey")
        );
        println!();
        println!("  How would you like to authenticate?");
        println!(
            "    {}  API Key    {}",
            cyan("1"),
            dim("— standard, recommended")
        );
        println!(
            "    {}  Gemini CLI {}",
            cyan("2"),
            dim("— reuse existing ~/.gemini OAuth session")
        );
        println!();
        let auth = prompt_choice("  Choice", 2, 1);
        if auth == 2 {
            println!();
            println!(
                "  {}  {}",
                warn_icon(),
                yellow("Gemini CLI OAuth is an unofficial integration.")
            );
            println!(
                "  {}  Some users have reported account restrictions. Proceed at own risk.",
                dim("  ")
            );
            print!("\n  Continue? {}: ", dim("(y/N)"));
            io::stdout().flush()?;
            let mut c = String::new();
            io::stdin().read_line(&mut c)?;
            if c.trim().eq_ignore_ascii_case("y") {
                println!("  {}  Using Gemini CLI OAuth", ok());
                "cli_oauth".to_string()
            } else {
                println!("  {}  Falling back to API key mode", skip());
                prompt_secret(&format!("  {} API key", bold("Gemini")))?
            }
        } else {
            prompt_secret(&format!("  {} API key", bold("Gemini")))?
        }
    } else {
        println!(
            "  {}  Get your key at {}",
            info(),
            cyan(&provider_docs_url(&provider.name))
        );
        println!();
        prompt_secret(&format!("  {} API key", bold(&capitalize(&provider.name))))?
    };
    println!();

    // ── Free model discovery ─────────────────────────────────────────
    let mut selected_default_model = provider.default_model.clone();
    let mut kilocode_fallback_models: Vec<String> = Vec::new();

    if provider.name == "opencode" {
        spin_start("Fetching OpenCode free models");
        match fetch_opencode_free_models(&api_key) {
            Ok(models) if !models.is_empty() => {
                spin_done(&format!("{} free model(s) found", models.len()));
                println!();
                let default_idx = preferred_opencode_model_index(&models);
                list_models_simple(&models, default_idx);
                let choice =
                    prompt_choice("  Select default model", models.len(), default_idx + 1) - 1;
                selected_default_model = models[choice].clone();
            }
            Ok(_) => {
                spin_warn(&format!(
                    "No free models found — using {}",
                    provider.default_model
                ));
            }
            Err(e) => {
                spin_warn(&format!("Could not reach OpenCode ({})", e));
            }
        }
    } else if provider.name == "openrouter" {
        spin_start("Fetching OpenRouter free models (≥8K context)");
        match fetch_openrouter_free_models(&api_key) {
            Ok(models) if !models.is_empty() => {
                spin_done(&format!("{} free model(s) found", models.len()));
                println!();
                let default_idx = preferred_openrouter_model_index(&models);
                println!(
                    "  {}",
                    dim(&format!("{:<3}  {:<52}  {}", "№", "Model", "Context"))
                );
                println!("  {}", dim(&"─".repeat(66)));
                for (i, m) in models.iter().enumerate() {
                    let label = format_openrouter_model(m);
                    let hint = if i == default_idx {
                        format!("  {}", green("← recommended"))
                    } else {
                        String::new()
                    };
                    println!(
                        "  {}  {}{}",
                        cyan(&format!("{:3}", i + 1)),
                        label,
                        dim(&hint)
                    );
                }
                println!();
                let choice =
                    prompt_choice("  Select default model", models.len(), default_idx + 1) - 1;
                selected_default_model = models[choice].id.clone();
                println!(
                    "  {}  {} {}",
                    ok(),
                    bold(&selected_default_model),
                    dim(&format!("({}K ctx)", models[choice].context_length / 1_000))
                );
            }
            Ok(_) => {
                spin_warn(&format!(
                    "No suitable free models — using {}",
                    provider.default_model
                ));
            }
            Err(e) => {
                spin_warn(&format!(
                    "Could not reach OpenRouter ({}) — using default",
                    e
                ));
            }
        }
    } else if provider.name == "ollama" {
        spin_start("Fetching Ollama local models");
        match fetch_ollama_models() {
            Ok(models) if !models.is_empty() => {
                spin_done(&format!("{} model(s) found", models.len()));
                println!();
                // Find qwen if possible as user requested qwen
                let default_idx = models.iter().position(|m| m.contains("qwen")).unwrap_or(0);
                list_models_simple(&models, default_idx);
                let choice = prompt_choice("  Select model", models.len(), default_idx + 1) - 1;
                selected_default_model = models[choice].clone();
            }
            _ => {
                spin_warn("No local models found — you may need to run `ollama pull llama3.2` first");
            }
        }
    } else if provider.name == "lmstudio" {
        spin_start("Fetching LM Studio local models");
        match fetch_lmstudio_models() {
            Ok(models) if !models.is_empty() => {
                spin_done(&format!("{} model(s) found", models.len()));
                println!();
                let default_idx = 0;
                list_models_simple(&models, default_idx);
                let choice = prompt_choice("  Select model", models.len(), default_idx + 1) - 1;
                selected_default_model = models[choice].clone();
            }
            _ => {
                spin_warn("No local models found — make sure a model is loaded in LM Studio");
            }
        }
    } else if provider.name == "kilocode" {
        spin_start("Fetching Kilo.ai free models");
        let free_models = match fetch_kilocode_free_models(&api_key) {
            Ok(models) if !models.is_empty() => {
                spin_done(&format!("{} free model(s) found", models.len()));
                models
            }
            Ok(_) => {
                spin_warn("No free models found — using built-in list");
                DEFAULT_FREE_MODELS.iter().map(|s| s.to_string()).collect()
            }
            Err(e) => {
                spin_warn(&format!(
                    "Could not reach Kilo.ai ({}) — using built-in list",
                    e
                ));
                DEFAULT_FREE_MODELS.iter().map(|s| s.to_string()).collect()
            }
        };
        println!();
        let default_idx = preferred_kilocode_model_index(&free_models);
        list_models_simple(&free_models, default_idx);
        let choice =
            prompt_choice("  Select default model", free_models.len(), default_idx + 1) - 1;
        selected_default_model = free_models[choice].clone();

        kilocode_fallback_models = free_models
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != choice)
            .map(|(_, m)| m.clone())
            .collect();
        kilocode_fallback_models.insert(0, selected_default_model.clone());
    }

    println!(
        "  {}  Provider {}  Model {}",
        ok(),
        green(&format!("[{}]", provider.name)),
        cyan(&selected_default_model)
    );
    println!();

    // ══════════════════════════════════════════════════════════════════
    // STEP 2 — Memory
    // ══════════════════════════════════════════════════════════════════
    section_header(
        2,
        "Memory",
        "How should your agent remember things between sessions?",
    );
    println!();
    println!(
        "  {}  {:<12}  {}",
        cyan("1"),
        bold("sqlite"),
        dim("Fast, searchable — recommended")
    );
    println!(
        "  {}  {:<12}  {}",
        cyan("2"),
        bold("markdown"),
        dim("Human-readable MEMORY.md file")
    );
    println!(
        "  {}  {:<12}  {}",
        cyan("3"),
        bold("none"),
        dim("No persistence (ephemeral)")
    );
    println!();

    let mem_choice = prompt_choice("  Choice", 3, 1);
    let memory_backend = match mem_choice {
        1 => "sqlite",
        2 => "markdown",
        3 => "none",
        _ => "sqlite",
    };
    println!(
        "  {}  Memory backend set to {}",
        ok(),
        green(&format!("[{}]", memory_backend))
    );

    let mut embed_provider = None;
    let mut embed_key = None;
    let mut embed_model = None;

    if memory_backend == "sqlite" {
        println!();
        print!(
            "  {}  Enable semantic search with vector embeddings? {}: ",
            info(),
            dim("(y/N)")
        );
        io::stdout().flush()?;
        let mut emb_in = String::new();
        io::stdin().read_line(&mut emb_in)?;
        if emb_in.trim().eq_ignore_ascii_case("y") {
            println!();
            println!(
                "  {}  {:<14}  {}",
                cyan("1"),
                bold("huggingface"),
                dim("Free, local — recommended (Qwen/Qwen3-Embedding-0.6B)")
            );
            println!(
                "  {}  {:<14}  {}",
                cyan("2"),
                bold("openai"),
                dim("$0.02 / 1M tokens (text-embedding-3-small)")
            );
            println!(
                "  {}  {:<14}  {}",
                cyan("3"),
                bold("gemini"),
                dim("Free tier available (text-embedding-004)")
            );
            println!();
            let ep_choice = prompt_choice("  Embedding provider", 3, 1);
            let (ep_name, ep_model) = match ep_choice {
                1 => ("huggingface", "Qwen/Qwen3-Embedding-0.6B"),
                2 => ("openai", "text-embedding-3-small"),
                3 => ("gemini", "models/text-embedding-004"),
                _ => ("huggingface", "Qwen/Qwen3-Embedding-0.6B"),
            };

            let key = if ep_name == provider.name {
                api_key.clone()
            } else {
                println!();
                prompt_secret(&format!(
                    "  {} API key for embeddings",
                    bold(&capitalize(ep_name))
                ))?
            };

            embed_provider = Some(ep_name.to_string());
            embed_key = Some(key);
            embed_model = Some(ep_model.to_string());
            println!(
                "  {}  Embeddings enabled via {}",
                ok(),
                green(&format!("[{}]", ep_name))
            );
        }
    }
    println!();

    // ══════════════════════════════════════════════════════════════════
    // STEP 3 — Voice (Groq Whisper)
    // ══════════════════════════════════════════════════════════════════
    section_header(
        3,
        "Voice Transcription",
        "Auto-transcribe voice messages via Groq Whisper",
    );
    println!();
    println!(
        "  {}  Free API key at {}",
        info(),
        cyan("https://console.groq.com")
    );
    println!();
    print!("  Enable voice transcription? {}: ", dim("(y/N)"));
    io::stdout().flush()?;
    let mut voice_in = String::new();
    io::stdin().read_line(&mut voice_in)?;

    let groq_key = if voice_in.trim().eq_ignore_ascii_case("y") {
        let key = prompt_secret("  Groq API key")?;
        if key.is_empty() {
            println!("  {}  No key provided — skipping voice", skip());
            String::new()
        } else {
            println!("  {}  Voice enabled {}", ok(), dim("(whisper-large-v3)"));
            key
        }
    } else {
        println!("  {}  Skipped — add later via config.json", skip());
        String::new()
    };
    println!();

    // ══════════════════════════════════════════════════════════════════
    // STEP 4 — Telegram
    // ══════════════════════════════════════════════════════════════════
    section_header(4, "Telegram Bot", "Chat with your agent from your phone");
    println!();
    println!(
        "  {}  Create a bot at {}",
        info(),
        cyan("https://t.me/BotFather")
    );
    println!();
    print!("  Set up Telegram? {}: ", dim("(y/N)"));
    io::stdout().flush()?;
    let mut tg_in = String::new();
    io::stdin().read_line(&mut tg_in)?;

    let telegram_config = if tg_in.trim().eq_ignore_ascii_case("y") {
        let token = prompt_secret("  Bot token (from @BotFather)")?;
        let username = prompt_with_default("  Your Telegram username", "@you")?;
        println!("  {}  Telegram bot configured", ok());
        if groq_key.is_empty() {
            println!(
                "  {}  {}",
                info(),
                dim("Tip: add a Groq key later to enable voice messages in Telegram!")
            );
        } else {
            println!(
                "  {}  Voice messages will be auto-transcribed in Telegram",
                ok()
            );
        }
        Some((token, username))
    } else {
        println!("  {}  Skipped", skip());
        None
    };
    println!();

    // ══════════════════════════════════════════════════════════════════
    // STEP 5 — Composio
    // ══════════════════════════════════════════════════════════════════
    section_header(
        5,
        "Composio Tools",
        "Connect external apps (Gmail, Slack, GitHub…)",
    );
    println!();
    println!(
        "  {}  API key at {}",
        info(),
        cyan("https://app.composio.dev")
    );
    println!();
    print!("  Enable Composio? {}: ", dim("(y/N)"));
    io::stdout().flush()?;
    let mut comp_in = String::new();
    io::stdin().read_line(&mut comp_in)?;

    let (composio_enabled, composio_api_key, composio_entity_id) =
        if comp_in.trim().eq_ignore_ascii_case("y") {
            let key = prompt_secret("  Composio API key")?;
            if key.is_empty() {
                println!("  {}  No key provided — skipping Composio", skip());
                (false, None, "default".to_string())
            } else {
                let entity_id = prompt_with_default("  Entity ID", "default")?;
                println!("  {}  Composio configured", ok());
                (true, Some(key), entity_id)
            }
        } else {
            println!("  {}  Skipped", skip());
            (false, None, "default".to_string())
        };
    println!();

    println!();

    // ══════════════════════════════════════════════════════════════════
    // STEP 6 — Brave Search
    // ══════════════════════════════════════════════════════════════════
    section_header(
        6,
        "Web Search",
        "Enable Brave Search for high-quality web results",
    );
    println!();
    println!(
        "  {}  Get your key at {}",
        info(),
        cyan("https://api.search.brave.com/app/keys")
    );
    println!();
    print!("  Enable Brave Search? {}: ", dim("(y/N)"));
    io::stdout().flush()?;
    let mut brave_in = String::new();
    io::stdin().read_line(&mut brave_in)?;

    let brave_api_key = if brave_in.trim().eq_ignore_ascii_case("y") {
        let key = prompt_secret("  Brave Search API key")?;
        if key.is_empty() {
            println!("  {}  No key provided — skipping", skip());
            None
        } else {
            println!("  {}  Brave Search enabled", ok());
            Some(key)
        }
    } else {
        println!("  {}  Skipped", skip());
        None
    };
    println!();

    // ══════════════════════════════════════════════════════════════════
    // STEP 7 — WhatsApp (Native Bridge)
    // ══════════════════════════════════════════════════════════════════
    section_header(
        7,
        "WhatsApp Native",
        "Chat via local WhatsApp bridge (whatsmeow)",
    );
    println!();
    print!("  Enable WhatsApp? {}: ", dim("(y/N)"));
    io::stdout().flush()?;
    let mut wa_in = String::new();
    io::stdin().read_line(&mut wa_in)?;

    let whatsapp_native_config = if wa_in.trim().eq_ignore_ascii_case("y") {
        println!(
            "  {}  OpenPaw will auto-start the bridge. Scan the QR code in your terminal to pair.",
            info()
        );
        let username = prompt_with_default("  Your Phone Number (e.g. +123...)", "+1...")?;

        println!("  {}  WhatsApp Native configured", ok());
        Some(("http://localhost:18790".to_string(), username))
    } else {
        println!("  {}  Skipped", skip());
        None
    };
    println!();

    // ══════════════════════════════════════════════════════════════════
    // STEP 8 — Pushover
    // ══════════════════════════════════════════════════════════════════
    section_header(
        8,
        "Pushover Notifications",
        "Get alerts on your phone when tasks finish",
    );
    println!();
    println!(
        "  {}  Get your token and user key at {}",
        info(),
        cyan("https://pushover.net")
    );
    println!();
    print!("  Enable Pushover? {}: ", dim("(y/N)"));
    io::stdout().flush()?;
    let mut push_in = String::new();
    io::stdin().read_line(&mut push_in)?;

    let pushover_config = if push_in.trim().eq_ignore_ascii_case("y") {
        let token = prompt_secret("  Pushover Application Token")?;
        let user_key = prompt_secret("  Pushover User Key")?;
        if token.is_empty() || user_key.is_empty() {
            println!("  {}  Missing keys — skipping", skip());
            None
        } else {
            println!("  {}  Pushover enabled", ok());
            Some((token, user_key))
        }
    } else {
        println!("  {}  Skipped", skip());
        None
    };
    println!();

    // ── Write files ──────────────────────────────────────────────────
    if !dir.exists() {
        fs::create_dir_all(dir).context("Failed to create workspace directory")?;
    }

    let config = generate_config(
        provider,
        &selected_default_model,
        &api_key,
        telegram_config.as_ref(),
        whatsapp_native_config.as_ref(),
        memory_backend,
        &groq_key,
        embed_provider.as_deref(),
        embed_key.as_deref(),
        embed_model.as_deref(),
        composio_enabled,
        composio_api_key.as_deref(),
        &composio_entity_id,
        &kilocode_fallback_models,
        brave_api_key.as_deref(),
        pushover_config.as_ref(),
    );
    fs::write(dir.join("config.json"), config)?;
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
        green("✅  Workspace ready!"),
        cyan("                                            │")
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
    println!("  {}  What's configured:", bold("Summary"));
    summary_row(
        "AI provider",
        &format!("{} / {}", provider.name, selected_default_model),
    );
    summary_row("Memory", memory_backend);
    if !groq_key.is_empty() {
        summary_row("Voice", "Groq Whisper (whisper-large-v3)");
    }
    if telegram_config.is_some() {
        summary_row("Telegram", "bot configured");
    }
    if whatsapp_native_config.is_some() {
        summary_row("WhatsApp Native", "configured");
    }
    if composio_enabled {
        summary_row("Composio", "enabled");
    }
    if pushover_config.is_some() {
        summary_row("Pushover", "enabled");
    }
    println!();
    println!("  {}  Next steps:", bold("→"));
    println!(
        "     {}  Edit {}  to give your agent a personality",
        dim("1."),
        cyan("SOUL.md")
    );
    println!(
        "     {}  Run  {}  for interactive chat",
        dim("2."),
        green("openpaw agent")
    );
    println!(
        "     {}  Run  {}  for a one-shot message",
        dim("3."),
        green("openpaw agent -m \"hi\"")
    );
    if telegram_config.is_some() {
        println!(
            "     {}  Your Telegram bot is live — send it a message!",
            dim("4.")
        );
    }
    println!();
    println!(
        "  {}  Use {} to see all commands.",
        info(),
        green("openpaw --help")
    );
    println!();

    Ok(())
}

// ── Config generation ─────────────────────────────────────────────

fn generate_config(
    provider: &ProviderConfig,
    default_model: &str,
    api_key: &str,
    telegram: Option<&(String, String)>,
    whatsapp_native: Option<&(String, String)>,
    memory_backend: &str,
    groq_key: &str,
    embed_provider: Option<&str>,
    embed_key: Option<&str>,
    embed_model: Option<&str>,
    composio_enabled: bool,
    composio_api_key: Option<&str>,
    composio_entity_id: &str,
    kilocode_fallback_models: &[String],
    brave_api_key: Option<&str>,
    pushover: Option<&(String, String)>,
) -> String {
    use serde_json::json;

    let mut providers = json!({
        provider.name.clone(): {
            "api_key": api_key,
        }
    });

    if let Some(url) = &provider.base_url {
        providers[provider.name.clone()]["base_url"] = json!(url);
    }

    if !kilocode_fallback_models.is_empty() {
        providers[provider.name.clone()]["fallback_models"] = json!(kilocode_fallback_models);
    }

    if let (Some(ep), Some(ek)) = (embed_provider, embed_key) {
        if ep != provider.name {
            providers[ep] = json!({ "api_key": ek });
        }
    }

    let telegram_vec = match telegram {
        Some((token, username)) => json!([{
            "account_id": "main",
            "bot_token": token,
            "allow_from": [username],
            "group_policy": "allowlist"
        }]),
        None => json!([]),
    };

    let whatsapp_vec = match whatsapp_native {
        Some((url, phone)) => json!([{
            "account_id": "main",
            "bridge_url": url,
            "allow_from": [phone],
            "auto_start": true
        }]),
        None => json!([]),
    };

    let mut config = json!({
        "default_provider": provider.name,
        "default_model": default_model,
        "models": { "providers": providers },
        "channels": {
            "telegram": telegram_vec,
            "whatsapp_native": whatsapp_vec
        },
        "memory": { "backend": memory_backend },
        "composio": {
            "enabled": composio_enabled,
            "api_key": composio_api_key,
            "entity_id": composio_entity_id
        },
        "http_request": {
            "enabled": true,
            "search_provider": if brave_api_key.is_some() { "brave" } else { "duckduckgo" },
            "brave_search_api_key": brave_api_key
        },
        "pushover": {
            "enabled": pushover.is_some(),
            "token": pushover.map(|p| p.0.clone()),
            "user_key": pushover.map(|p| p.1.clone())
        }
    });

    if let Some(m) = embed_model {
        config["memory"]["embedding_model"] = json!(m);
    }

    if !groq_key.is_empty() {
        config["voice"] = json!({
            "provider": "groq",
            "api_key": groq_key,
            "model": "whisper-large-v3"
        });
    }

    serde_json::to_string_pretty(&config).unwrap_or_default()
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
