use crate::channel_catalog;
use crate::config::Config;
use crate::version;
use std::io::{self, Write};

pub fn run(cfg: &Config) -> anyhow::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();

    writeln!(handle, "openpaw Status\n")?;
    writeln!(handle, "Version:     {}", version::VERSION)?;
    // Placeholder for workspace and config path, matching zig's basic info
    writeln!(handle, "Workspace:   (current directory)")?;
    writeln!(handle, "Config:      (loaded configuration)\n")?;

    writeln!(
        handle,
        "Temperature: {:.1}\n",
        cfg.default_temperature.unwrap_or(0.7)
    )?;

    writeln!(handle, "Memory:      {}", cfg.memory.backend)?;
    writeln!(
        handle,
        "Browser:     {}",
        if cfg.browser.enabled {
            "enabled"
        } else {
            "disabled"
        }
    )?;
    writeln!(
        handle,
        "Composio:    {}",
        if cfg.composio.enabled {
            "enabled"
        } else {
            "disabled"
        }
    )?;
    writeln!(
        handle,
        "Hardware:    {}",
        if cfg.hardware.enabled {
            "enabled"
        } else {
            "disabled"
        }
    )?;
    writeln!(
        handle,
        "HTTP Req:    {}\n",
        if cfg.http_request.enabled {
            "enabled"
        } else {
            "disabled"
        }
    )?;

    writeln!(
        handle,
        "Gateway:     {}:{}\n",
        cfg.gateway.host, cfg.gateway.port
    )?;

    writeln!(handle, "Channels:")?;
    for meta in channel_catalog::KNOWN_CHANNELS {
        let status_text = if meta.id == channel_catalog::ChannelId::Cli {
            "always".to_string()
        } else if channel_catalog::is_configured(cfg, meta.id) {
            meta.configured_message.to_string()
        } else {
            "not configured".to_string()
        };
        writeln!(handle, "  {}: {}", meta.label, status_text)?;
    }

    writeln!(handle, "")?;
    handle.flush()?;

    Ok(())
}
