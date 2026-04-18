use crate::config::Config;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Ok,
    Warn,
    Err,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagItem {
    pub severity: Severity,
    pub category: String,
    pub message: String,
}

impl DiagItem {
    pub fn ok(cat: &str, msg: &str) -> Self {
        Self {
            severity: Severity::Ok,
            category: cat.to_string(),
            message: msg.to_string(),
        }
    }
    pub fn warn(cat: &str, msg: &str) -> Self {
        Self {
            severity: Severity::Warn,
            category: cat.to_string(),
            message: msg.to_string(),
        }
    }
    pub fn err(cat: &str, msg: &str) -> Self {
        Self {
            severity: Severity::Err,
            category: cat.to_string(),
            message: msg.to_string(),
        }
    }

    pub fn icon(&self) -> String {
        match self.severity {
            Severity::Ok => "[ok]".to_string(),
            Severity::Warn => "[warn]".to_string(),
            Severity::Err => "[ERR]".to_string(),
        }
    }

    pub fn icon_colored(&self) -> String {
        // Plain output since colored is not available
        self.icon()
    }
}

pub struct Doctor;

impl Doctor {
    pub fn run(config: &Config, writer: &mut dyn Write, _color: bool) -> Result<()> {
        let mut items = Vec::new();

        Self::check_config_semantics(config, &mut items)?;
        Self::check_security_defaults(config, &mut items)?;
        Self::check_workspace(config, &mut items)?;
        Self::check_environment(&mut items)?;

        writeln!(writer, "openpaw Doctor (enhanced)\n")?;

        let mut current_cat = String::new();
        let mut ok_count = 0;
        let mut warn_count = 0;
        let mut err_count = 0;

        for item in &items {
            if item.category != current_cat {
                current_cat = item.category.clone();
                writeln!(writer, "  [{}]", current_cat)?;
            }
            let ic = item.icon();
            writeln!(writer, "    {} {}", ic, item.message)?;

            match item.severity {
                Severity::Ok => ok_count += 1,
                Severity::Warn => warn_count += 1,
                Severity::Err => err_count += 1,
            }
        }

        writeln!(
            writer,
            "\nSummary: {} ok, {} warnings, {} errors",
            ok_count, warn_count, err_count
        )?;

        if err_count > 0 {
            writeln!(writer, "Run 'openpaw doctor --fix' or check your config.")?;
        }

        Ok(())
    }

    fn check_config_semantics(config: &Config, items: &mut Vec<DiagItem>) -> Result<()> {
        let cat = "config";
        // Provider / API key
        if config.default_provider.is_empty() {
            items.push(DiagItem::err(cat, "no default_provider configured"));
        } else {
            items.push(DiagItem::ok(
                cat,
                &format!("provider: {}", config.default_provider),
            ));
        }

        // Default model must be resolvable
        if config.default_model.is_none() {
            if let Some(ref models) = config.models {
                if !models.providers.contains_key(&config.default_provider) {
                    items.push(DiagItem::warn(
                        cat,
                        &format!(
                            "no model config found for provider '{}'",
                            config.default_provider
                        ),
                    ));
                } else {
                    items.push(DiagItem::ok(cat, "provider config found"));
                }
            }
        } else {
            items.push(DiagItem::ok(
                cat,
                &format!("model: {}", config.default_model.as_deref().unwrap_or("(default)")),
            ));
        }

        Ok(())
    }

    // ── 1.4.1: Security default checks ─────────────────────────────────────
    fn check_security_defaults(config: &Config, items: &mut Vec<DiagItem>) -> Result<()> {
        let cat = "security";

        // Gateway: auth token
        if config.gateway.token.is_none() {
            items.push(DiagItem::warn(
                cat,
                "gateway.token is not set — /api/config is unauthenticated; \
                 set a token for any non-localhost deployment",
            ));
        } else {
            items.push(DiagItem::ok(cat, "gateway auth token configured"));
        }

        // Gateway: 0.0.0.0 without auth
        let public_bind = config.gateway.allow_public_bind.unwrap_or(false);
        if public_bind && config.gateway.token.is_none() {
            items.push(DiagItem::err(
                cat,
                "gateway binds to 0.0.0.0 (allow_public_bind=true) but no auth token is set — \
                 API is publicly accessible without any authentication",
            ));
        } else if public_bind {
            items.push(DiagItem::warn(
                cat,
                "gateway binds to 0.0.0.0; ensure firewall rules restrict access",
            ));
        } else {
            items.push(DiagItem::ok(cat, "gateway binds to 127.0.0.1 (localhost only)"));
        }

        // Telegram: open allowlists
        for tg in &config.channels.telegram {
            if tg.allow_from.is_empty() {
                items.push(DiagItem::err(
                    cat,
                    &format!(
                        "Telegram account '{}': allow_from is empty — bot accepts messages from ANY user",
                        tg.account_id
                    ),
                ));
            } else {
                items.push(DiagItem::ok(
                    cat,
                    &format!(
                        "Telegram account '{}': allow_from restricted to {} user(s)",
                        tg.account_id,
                        tg.allow_from.len()
                    ),
                ));
            }
        }

        // Workspace must be writable
        let ws = std::path::Path::new(&config.workspace_dir);
        if ws.exists() {
            // Create + delete a temp file to test writability
            let test_path = ws.join(".openpaw_write_test");
            match std::fs::write(&test_path, b"test") {
                Ok(_) => {
                    let _ = std::fs::remove_file(&test_path);
                    items.push(DiagItem::ok(cat, "workspace directory is writable"));
                }
                Err(e) => {
                    items.push(DiagItem::err(
                        cat,
                        &format!("workspace directory is not writable: {}", e),
                    ));
                }
            }
        }

        Ok(())
    }

    fn check_workspace(config: &Config, items: &mut Vec<DiagItem>) -> Result<()> {
        let cat = "workspace";
        let ws_path = std::path::Path::new(&config.workspace_dir);
        if ws_path.exists() {
            if ws_path.is_dir() {
                items.push(DiagItem::ok(cat, &format!("found: {:?}", ws_path)));
            } else {
                items.push(DiagItem::err(
                    cat,
                    &format!("not a directory: {:?}", ws_path),
                ));
            }
        } else {
            items.push(DiagItem::err(cat, &format!("missing: {:?}", ws_path)));
        }
        Ok(())
    }

    fn check_environment(items: &mut Vec<DiagItem>) -> Result<()> {
        let cat = "env";
        // Check git
        if which::which("git").is_ok() {
            items.push(DiagItem::ok(cat, "git found"));
        } else {
            items.push(DiagItem::warn(cat, "git not found"));
        }
        // Check curl
        if which::which("curl").is_ok() {
            items.push(DiagItem::ok(cat, "curl found"));
        } else {
            items.push(DiagItem::warn(cat, "curl not found"));
        }
        // Check Chrome/CDP
        let chrome_found = dirs::home_dir()
            .map(|h| {
                h.join("AppData")
                    .join("Local")
                    .join("Google")
                    .join("Chrome")
                    .join("Application")
                    .join("chrome.exe")
            })
            .map(|p| p.exists())
            .unwrap_or(false)
            || std::path::Path::new(r"C:\Program Files\Google\Chrome\Application\chrome.exe")
                .exists()
            || std::path::Path::new(
                r"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            )
            .exists()
            || which::which("google-chrome").is_ok()
            || which::which("google-chrome-stable").is_ok()
            || which::which("chromium-browser").is_ok();
        if chrome_found {
            items.push(DiagItem::ok(cat, "Chrome/Chromium found (CDP browser)"));
        } else {
            items.push(DiagItem::warn(
                cat,
                "Chrome/Chromium not found (required for CDP browser tool)",
            ));
        }
        Ok(())
    }
}
