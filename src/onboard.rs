use crate::workspace_templates::*;
use anyhow::{Context, Result};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

// ── ASCII Art Banner ─────────────────────────────────────────────

pub const BANNER: &str = r#"
   ___                 ____
  / _ \ _ __   ___ _ __|  _ \ __ ___      __
 | | | | '_ \ / _ \ '_ \ |_) / _` \ \ /\ / /
 | |_| | |_) |  __/ | | |  __/ (_| |\ V  V /
  \___/| .__/ \___|_| |_|_|   \__,_| \_/\_/
       |_|

"#;

const DIVIDER: &str = "────────────────────────────────────────";
const THIN_DIV: &str = "  · · · · · · · · · · · · · · · · · ·";

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

    // ── Header ──────────────────────────────────────────────────
    print!("{}", BANNER);
    println!("  Welcome to OpenPaw — your lightweight local AI agent.");
    println!("  Powered by Rust  •  Zero cloud APIs required for core features");
    println!("  {}", DIVIDER);
    println!();

    // ── Already initialized? ────────────────────────────────────
    if dir.join("config.json").exists() {
        print!("  ⚠  Config already exists in this directory.\n  Overwrite everything? (y/N): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("  Cancelled. Your existing config is untouched.");
            return Ok(());
        }
        println!();
    }

    // ═══════════════════════════════════════════════════════════
    // STEP 1 — AI Provider
    // ═══════════════════════════════════════════════════════════
    section_header(1, "AI Provider");

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
            default_model: "anthropic/claude-3.5-sonnet".to_string(),
            base_url: Some("https://openrouter.ai/api/v1".to_string()),
        },
    ];

    for (i, p) in providers.iter().enumerate() {
        let note = match p.name.as_str() {
            "gemini" => "free tier available",
            "openai" => "gpt-4o, o1, o3...",
            "anthropic" => "claude-3.5-sonnet, claude-3.7...",
            "openrouter" => "access 200+ models via one key",
            _ => "",
        };
        println!("    {}. {:12}  {}", i + 1, p.name, dim(note));
    }

    let provider_idx = prompt_number("\n  Choice", 1, providers.len(), 1) - 1;
    let provider = &providers[provider_idx];

    let api_key = if provider.name == "gemini" {
        println!("\n  How would you like to authenticate with Gemini?");
        println!("    1. API Key      (Standard)");
        println!("    2. Gemini CLI   (Reuse existing ~/.gemini OAuth login)");

        let auth_choice = prompt_number("  Choice", 1, 2, 1);
        if auth_choice == 2 {
            println!("  {} Using Gemini CLI OAuth", tick());
            String::new() // Provider will auto-detect from ~/.gemini
        } else {
            prompt_secret("\n  Gemini API key")?
        }
    } else {
        prompt_secret(&format!("\n  {} API key", capitalize(&provider.name)))?
    };

    println!(
        "  {} Default model: {}",
        tick(),
        dim(&provider.default_model)
    );
    println!();

    // ═══════════════════════════════════════════════════════════
    // STEP 2 — Identity
    // ═══════════════════════════════════════════════════════════
    section_header(2, "Identity");

    let agent_name = prompt_with_default("  Agent name", "OpenPaw")?;
    let user_name = prompt_with_default("  Your name ", "User")?;
    let timezone = prompt_with_default("  Timezone  ", "UTC")?;
    println!();

    // ═══════════════════════════════════════════════════════════
    // STEP 3 — Memory backend
    // ═══════════════════════════════════════════════════════════
    section_header(3, "Memory");
    println!("  How should your agent remember things between sessions?");
    println!();
    println!(
        "    1. sqlite    {} fast, searchable, recommended",
        dim("→")
    );
    println!(
        "    2. markdown  {} human-readable MEMORY.md file",
        dim("→")
    );
    println!("    3. none      {} no persistence (ephemeral)", dim("→"));
    println!();

    let mem_choice = prompt_number("  Choice", 1, 3, 1);
    let memory_backend = match mem_choice {
        1 => "sqlite",
        2 => "markdown",
        3 => "none",
        _ => "sqlite",
    };
    println!("  {} Memory backend: {}", tick(), memory_backend);

    let mut embed_provider = None;
    let mut embed_key = None;
    let mut embed_model = None;

    if memory_backend == "sqlite" {
        print!("\n  Enable semantic search (vector embeddings)? (y/N): ");
        io::stdout().flush()?;
        let mut embed_input = String::new();
        io::stdin().read_line(&mut embed_input)?;
        if embed_input.trim().to_lowercase() == "y" {
            println!("\n    Select embedding provider:");
            println!("      1. huggingface (free, recommended: Qwen)");
            println!("      2. openai      (requires key, 0.02c / 1m tokens)");
            println!("      3. gemini      (requires key, free tier available)");

            let ep_choice = prompt_number("    Choice", 1, 3, 1);
            let (ep_name, ep_default_model) = match ep_choice {
                1 => ("huggingface", "Qwen/Qwen3-Embedding-0.6B"),
                2 => ("openai", "text-embedding-3-small"),
                3 => ("gemini", "models/text-embedding-004"),
                _ => ("huggingface", "Qwen/Qwen3-Embedding-0.6B"),
            };

            let key = if ep_name == provider.name {
                api_key.clone()
            } else {
                prompt_secret(&format!("    {} API key", capitalize(ep_name)))?
            };

            embed_provider = Some(ep_name.to_string());
            embed_key = Some(key);
            embed_model = Some(ep_default_model.to_string());
            println!("    {} Embeddings enabled via {}", tick(), ep_name);
        }
    }
    println!();

    // ═══════════════════════════════════════════════════════════
    // STEP 4 — Voice (Groq Whisper)
    // ═══════════════════════════════════════════════════════════
    section_header(4, "Voice Transcription (optional)");
    println!("  OpenPaw can transcribe voice messages via Groq Whisper.");
    println!(
        "  Get a free API key at: {}",
        dim("https://console.groq.com")
    );
    println!();

    print!("  Enable voice transcription? (y/N): ");
    io::stdout().flush()?;
    let mut voice_input = String::new();
    io::stdin().read_line(&mut voice_input)?;
    let voice_input = voice_input.trim().to_lowercase();

    let groq_key = if voice_input == "y" {
        let key = prompt_secret("  Groq API key")?;
        if key.is_empty() {
            println!("  {} Skipping voice (no key provided)", dim("·"));
            String::new()
        } else {
            println!(
                "  {} Voice transcription enabled (whisper-large-v3)",
                tick()
            );
            key
        }
    } else {
        println!(
            "  {} Skipping — you can add this later in config.json",
            dim("·")
        );
        String::new()
    };
    println!();

    // ═══════════════════════════════════════════════════════════
    // STEP 5 — Telegram (optional)
    // ═══════════════════════════════════════════════════════════
    section_header(5, "Telegram Bot (optional)");
    println!("  Connect a Telegram bot to chat with your agent on mobile.");
    println!("  Create one at: {}", dim("https://t.me/BotFather"));
    println!();

    print!("  Set up Telegram? (y/N): ");
    io::stdout().flush()?;
    let mut tg_input = String::new();
    io::stdin().read_line(&mut tg_input)?;

    let telegram_config = if tg_input.trim().eq_ignore_ascii_case("y") {
        let token = prompt_secret("  Bot token (from @BotFather)")?;
        let username = prompt_with_default("  Your Telegram username (@yourname)", "@you")?;
        println!("  {} Telegram bot configured", tick());
        if groq_key.is_empty() {
            println!(
                "  {} Tip: add a Groq key later to use voice messages in Telegram!",
                dim("💡")
            );
        } else {
            println!(
                "  {} Voice messages in Telegram will be auto-transcribed!",
                tick()
            );
        }
        Some((token, username))
    } else {
        println!(
            "  {} Skipping — run `openpaw onboard` again to add it later",
            dim("·")
        );
        None
    };
    println!();

    // ═══════════════════════════════════════════════════════════
    // Write files
    // ═══════════════════════════════════════════════════════════
    if !dir.exists() {
        fs::create_dir_all(dir).context("Failed to create workspace directory")?;
    }

    let config = generate_config(
        provider,
        &api_key,
        telegram_config.as_ref(),
        memory_backend,
        &groq_key,
        embed_provider.as_deref(),
        embed_key.as_deref(),
        embed_model.as_deref(),
    );
    fs::write(dir.join("config.json"), config)?;

    let ctx = ProjectContext {
        user_name: user_name.clone(),
        timezone: timezone.clone(),
        agent_name: agent_name.clone(),
        communication_style: "Be warm, natural, and clear. Avoid robotic phrasing.".to_string(),
    };
    scaffold_workspace(dir, &ctx)?;

    // ═══════════════════════════════════════════════════════════
    // Done!
    // ═══════════════════════════════════════════════════════════
    println!("  {}", DIVIDER);
    println!();
    println!(
        "  ✅  Workspace ready at {}",
        dir.canonicalize()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| dir.display().to_string())
    );
    println!();
    println!("  {} What's set up:", tick());
    println!(
        "     • AI provider  →  {} / {}",
        provider.name, provider.default_model
    );
    println!("     • Memory       →  {}", memory_backend);
    if !groq_key.is_empty() {
        println!("     • Voice        →  Groq Whisper (whisper-large-v3)");
    }
    if telegram_config.is_some() {
        println!("     • Telegram     →  bot configured");
    }
    println!();
    println!("  {} Next steps:", dim("→"));
    println!("     1. Edit SOUL.md to customize your agent's personality");
    println!(
        "     2. Run:  openpaw agent          {}  interactive chat",
        dim("→")
    );
    println!(
        "     3. Run:  openpaw agent -m \"hi\"  {}  one-shot message",
        dim("→")
    );
    if telegram_config.is_some() {
        println!("     4. Your Telegram bot is ready — message it now!");
    }
    println!();
    println!("  {} Use `openpaw --help` to see all commands.", dim("💡"));
    println!();

    Ok(())
}

// ── Config generation ─────────────────────────────────────────────

fn generate_config(
    provider: &ProviderConfig,
    api_key: &str,
    telegram: Option<&(String, String)>,
    memory_backend: &str,
    groq_key: &str,
    embed_provider: Option<&str>,
    embed_key: Option<&str>,
    embed_model: Option<&str>,
) -> String {
    let base_url_field = match &provider.base_url {
        Some(url) => format!(",\n        \"base_url\": \"{}\"", url),
        None => String::new(),
    };

    let mut providers_json = format!(
        r#"      "{}": {{
        "api_key": "{}"{}
      }}"#,
        provider.name, api_key, base_url_field
    );

    if let (Some(ep), Some(ek)) = (embed_provider, embed_key) {
        if ep != provider.name {
            providers_json.push_str(&format!(
                ",\n      \"{}\": {{\n        \"api_key\": \"{}\"\n      }}",
                ep, ek
            ));
        }
    }

    let telegram_section = match telegram {
        Some((token, username)) => format!(
            r#""telegram": [
    {{
      "account_id": "main",
      "bot_token": "{}",
      "allow_from": ["{}"],
      "group_policy": "allowlist"
    }}
  ]"#,
            token, username
        ),
        None => r#""telegram": []"#.to_string(),
    };

    let voice_section = if !groq_key.is_empty() {
        format!(
            r#",
  "voice": {{
    "provider": "groq",
    "api_key": "{}",
    "model": "whisper-large-v3"
  }}"#,
            groq_key
        )
    } else {
        String::new()
    };

    format!(
        r#"{{
  "default_provider": "{provider}",
  "default_model": "{model}",
  "models": {{
    "providers": {{
{providers}
    }}
  }},
  "channels": {{
    {telegram}
  }},
  "memory": {{
    "backend": "{memory}"{memory_model}
  }},
  "http_request": {{
    "enabled": true,
    "search_provider": "duckduckgo"
  }}{voice}
}}"#,
        provider = provider.name,
        model = provider.default_model,
        providers = providers_json,
        telegram = telegram_section,
        memory = memory_backend,
        memory_model = match embed_model {
            Some(m) => format!(",\n    \"embedding_model\": \"{}\"", m),
            None => String::new(),
        },
        voice = voice_section,
    )
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

// ── UX helpers ────────────────────────────────────────────────────

fn section_header(n: u8, title: &str) {
    println!("  {} Step {}  {}  {}", dim("┌"), n, title, dim(THIN_DIV));
    println!();
}

fn tick() -> &'static str {
    "✓"
}

fn dim(s: &str) -> String {
    // ANSI dim on terminals that support it; falls back gracefully
    format!("\x1b[2m{}\x1b[0m", s)
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn prompt_with_default(label: &str, default: &str) -> Result<String> {
    print!("{} [{}]: ", label, dim(default));
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
    print!("{}: ", label);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn prompt_number(label: &str, min: usize, max: usize, default: usize) -> usize {
    print!("{} [{}-{}, default {}]: ", label, min, max, default);
    let _ = io::stdout().flush();
    let mut input = String::new();
    let _ = io::stdin().read_line(&mut input);
    let n: usize = input.trim().parse().unwrap_or(default);
    n.clamp(min, max)
}
