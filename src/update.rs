use anyhow::{anyhow, Result};
use reqwest::header;
use serde::Deserialize;
use std::env;
use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    Nix,
    Homebrew,
    Docker,
    Binary,
    Dev,
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub html_url: String,
    pub body: String,
}

#[derive(Debug, Default)]
pub struct UpdateOptions {
    pub check_only: bool,
    pub yes: bool,
}

pub struct Update;

impl Update {
    pub fn run(options: UpdateOptions) -> Result<()> {
        let current_version = env!("CARGO_PKG_VERSION");
        let install_method = Self::detect_install_method()?;

        match install_method {
            InstallMethod::Nix | InstallMethod::Homebrew | InstallMethod::Docker => {
                Self::print_package_manager_update(install_method);
                return Ok(());
            }
            InstallMethod::Dev => {
                println!("Development installation detected.");
                println!("To update, run:\n  git pull && cargo build --release");
                return Ok(());
            }
            InstallMethod::Binary | InstallMethod::Unknown => {
                // Proceed to check update
            }
        }

        let latest = Self::get_latest_release()?;
        let current_clean = current_version.trim_start_matches('v');
        let latest_clean = latest.tag_name.trim_start_matches('v');

        if current_clean == latest_clean {
            println!("Already up to date: {}", current_version);
            return Ok(());
        }

        println!("Current version: {}", current_version);
        println!("Latest version:  {}", latest.tag_name);
        println!();

        if !latest.body.is_empty() {
            println!("Release notes:");
            for line in latest.body.lines().take(5) {
                if !line.trim().is_empty() && !line.starts_with("##") {
                    println!("  {}", line);
                }
            }
            println!();
        }

        println!("Release: {}", latest.html_url);
        println!();

        if options.check_only {
            return Ok(());
        }

        if !options.yes {
            print!("Download and install {}? [y/N] ", latest.tag_name);
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let response = input.trim();
            if response != "y" && response != "Y" {
                println!("Update cancelled.");
                return Ok(());
            }
        }

        println!("Automatic binary update is not yet fully implemented in Rust port.");
        println!("Please download manually from: {}", latest.html_url);

        Ok(())
    }

    fn detect_install_method() -> Result<InstallMethod> {
        let exe_path = env::current_exe()?;
        let path_str = exe_path.to_string_lossy();

        if path_str.contains("/nix/store/") {
            Ok(InstallMethod::Nix)
        } else if path_str.contains("/homebrew/") || path_str.contains("/Cellar/") {
            Ok(InstallMethod::Homebrew)
        } else if path_str == "/openpaw" { // Assuming Docker path
            Ok(InstallMethod::Docker)
        } else if path_str.contains("target/release") || path_str.contains("target/debug") {
            Ok(InstallMethod::Dev)
        } else {
            Ok(InstallMethod::Binary)
        }
    }

    fn print_package_manager_update(method: InstallMethod) {
        let (name, cmd) = match method {
            InstallMethod::Nix => ("Nix", "nix-channel --update && nix-env -iA nixpkgs.openpaw"),
            InstallMethod::Homebrew => ("Homebrew", "brew upgrade openpaw"),
            InstallMethod::Docker => ("Docker", "docker pull ghcr.io/openpaw/openpaw:latest"),
            _ => ("Unknown", ""),
        };
        println!("Detected installation via: {}", name);
        println!("To update, run:\n  {}", cmd);
    }

    fn get_latest_release() -> Result<ReleaseInfo> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("openpaw/0.1")
            .build()?;

        let url = "https://api.github.com/repos/openpaw/openpaw/releases/latest";
        let response = client
            .get(url)
            .header(header::ACCEPT, "application/vnd.github.v3+json")
            .send()?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to fetch release info: {}", response.status()));
        }

        let info: ReleaseInfo = response.json()?;
        Ok(info)
    }
}
