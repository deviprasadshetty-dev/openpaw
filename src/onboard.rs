use crate::workspace_templates::*;
use anyhow::{Context, Result};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

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
            agent_name: "nullclaw".to_string(),
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

pub fn interactive_onboard<P: AsRef<Path>>(workspace_dir: P) -> Result<()> {
    let dir = workspace_dir.as_ref();
    println!("🐾 Welcome to OpenPaw!\n");

    // Check if already initialized
    if dir.join("config.json").exists() {
        print!("Config already exists. Overwrite? (y/N): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    // Provider selection
    let providers = vec![
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
            name: "gemini".to_string(),
            default_model: "gemini-2.0-flash".to_string(),
            base_url: Some("https://generativelanguage.googleapis.com/v1beta/openai/".to_string()),
        },
        ProviderConfig {
            name: "openrouter".to_string(),
            default_model: "anthropic/claude-3.5-sonnet".to_string(),
            base_url: Some("https://openrouter.ai/api/v1".to_string()),
        },
    ];

    println!("Select an AI provider:");
    for (i, provider) in providers.iter().enumerate() {
        println!("  {}. {}", i + 1, provider.name);
    }
    print!("\nChoice (1-4): ");
    io::stdout().flush()?;

    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    let choice: usize = choice.trim().parse().unwrap_or(1);
    let provider = providers.get(choice - 1).unwrap_or(&providers[0]);

    // API Key
    print!("Enter your {} API key: ", provider.name);
    io::stdout().flush()?;
    let mut api_key = String::new();
    io::stdin().read_line(&mut api_key)?;
    let api_key = api_key.trim().to_string();

    // Agent name
    print!("Agent name [OpenPaw]: ");
    io::stdout().flush()?;
    let mut agent_name = String::new();
    io::stdin().read_line(&mut agent_name)?;
    let agent_name = if agent_name.trim().is_empty() {
        "OpenPaw".to_string()
    } else {
        agent_name.trim().to_string()
    };

    // User name
    print!("Your name [User]: ");
    io::stdout().flush()?;
    let mut user_name = String::new();
    io::stdin().read_line(&mut user_name)?;
    let user_name = if user_name.trim().is_empty() {
        "User".to_string()
    } else {
        user_name.trim().to_string()
    };

    // Timezone
    print!("Timezone [UTC]: ");
    io::stdout().flush()?;
    let mut timezone = String::new();
    io::stdin().read_line(&mut timezone)?;
    let timezone = if timezone.trim().is_empty() {
        "UTC".to_string()
    } else {
        timezone.trim().to_string()
    };

    // Telegram setup (optional)
    print!("\nSet up Telegram? (y/N): ");
    io::stdout().flush()?;
    let mut setup_telegram = String::new();
    io::stdin().read_line(&mut setup_telegram)?;
    
    let telegram_config = if setup_telegram.trim().eq_ignore_ascii_case("y") {
        print!("Telegram Bot Token (from @BotFather): ");
        io::stdout().flush()?;
        let mut bot_token = String::new();
        io::stdin().read_line(&mut bot_token)?;
        
        print!("Your Telegram username (e.g., @yourname): ");
        io::stdout().flush()?;
        let mut username = String::new();
        io::stdin().read_line(&mut username)?;
        
        Some((bot_token.trim().to_string(), username.trim().to_string()))
    } else {
        None
    };

    // Create directory if needed
    if !dir.exists() {
        fs::create_dir_all(dir).context("Failed to create workspace directory")?;
    }

    // Generate config.json
    let config = generate_config(provider, &api_key, telegram_config.as_ref());
    fs::write(dir.join("config.json"), config)?;

    // Scaffold workspace files
    let ctx = ProjectContext {
        user_name: user_name.clone(),
        timezone: timezone.clone(),
        agent_name: agent_name.clone(),
        communication_style: "Be warm, natural, and clear. Avoid robotic phrasing.".to_string(),
    };
    scaffold_workspace(dir, &ctx)?;

    println!("\n✅ Workspace initialized at {}", dir.canonicalize()?.display());
    println!("\nNext steps:");
    println!("  1. Review SOUL.md and IDENTITY.md");
    println!("  2. Run: openpaw agent");

    Ok(())
}

fn generate_config(provider: &ProviderConfig, api_key: &str, telegram: Option<&(String, String)>) -> String {
    let base_url_line = if let Some(url) = &provider.base_url {
        format!("        \"base_url\": \"{}\",", url)
    } else {
        String::new()
    };

    let telegram_section = if let Some((token, username)) = telegram {
        format!(
            r#""telegram": [
      {{
        "account_id": "main",
        "bot_token": "{token}",
        "allow_from": ["{username}"],
        "group_policy": "allowlist"
      }}
    ]"#,
            token = token,
            username = username
        )
    } else {
        r#""telegram": []"#.to_string()
    };

    format!(
        r#"{{
  "default_provider": "{provider}",
  "default_model": "{model}",
  "models": {{
    "providers": {{
      "{provider}": {{
        "api_key": "{key}"{base_url_comma}
{base_url}
      }}
    }}
  }},
  "channels": {{
    {telegram}
  }},
  "http_request": {{
    "enabled": true,
    "search_provider": "duckduckgo"
  }},
  "memory": {{
    "backend": "sqlite"
  }}
}}"#,
        provider = provider.name,
        model = provider.default_model,
        key = api_key,
        base_url = base_url_line,
        base_url_comma = if base_url_line.is_empty() { "" } else { "," },
        telegram = telegram_section
    )
}

pub fn scaffold_workspace<P: AsRef<Path>>(workspace_dir: P, ctx: &ProjectContext) -> Result<()> {
    let dir = workspace_dir.as_ref();

    if !dir.exists() {
        fs::create_dir_all(dir).context("Failed to create workspace directory")?;
    }

    // Process templates that need string replacement
    let soul_content = SOUL_TEMPLATE
        .replace("{{agent_name}}", &ctx.agent_name)
        .replace("{{communication_style}}", &ctx.communication_style);

    let identity_content = IDENTITY_TEMPLATE.replace("{{agent_name}}", &ctx.agent_name);

    let user_content = USER_TEMPLATE
        .replace("{{user_name}}", &ctx.user_name)
        .replace("{{timezone}}", &ctx.timezone);

    // Write files if they don't exist
    write_if_missing(&dir.join("SOUL.md"), &soul_content)?;
    write_if_missing(&dir.join("AGENTS.md"), AGENTS_TEMPLATE)?;
    write_if_missing(&dir.join("TOOLS.md"), TOOLS_TEMPLATE)?;
    write_if_missing(&dir.join("IDENTITY.md"), &identity_content)?;
    write_if_missing(&dir.join("USER.md"), &user_content)?;
    write_if_missing(&dir.join("HEARTBEAT.md"), HEARTBEAT_TEMPLATE)?;

    // Write BOOTSTRAP.md
    write_if_missing(&dir.join("BOOTSTRAP.md"), BOOTSTRAP_TEMPLATE)?;

    Ok(())
}

fn write_if_missing(path: &Path, content: &str) -> Result<()> {
    if !path.exists() {
        fs::write(path, content).context(format!("Failed to write {}", path.display()))?;
    }
    Ok(())
}
