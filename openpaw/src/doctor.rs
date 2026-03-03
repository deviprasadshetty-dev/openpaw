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
        if config.default_provider.is_empty() {
            items.push(DiagItem::err(cat, "no default_provider configured"));
        } else {
            items.push(DiagItem::ok(
                cat,
                &format!("provider: {}", config.default_provider),
            ));
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
                items.push(DiagItem::err(cat, &format!("not a directory: {:?}", ws_path)));
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
        Ok(())
    }
}
