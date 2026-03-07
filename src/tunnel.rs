use anyhow::{Context, Result, anyhow};
use regex::Regex;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tracing::info;

#[derive(Debug, Clone)]
pub struct TunnelInfo {
    pub provider: String,
    pub public_url: String,
    pub local_port: u16,
}

pub struct TunnelHandle {
    pub info: TunnelInfo,
    child: Arc<Mutex<Child>>,
}

impl TunnelHandle {
    pub fn stop(&self) -> Result<()> {
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        child.kill().context("Failed to kill tunnel process")?;
        child.wait().context("Failed to wait for tunnel process")?;
        Ok(())
    }
}

pub fn start_tunnel(provider: &str, port: u16) -> Result<TunnelHandle> {
    match provider.to_lowercase().as_str() {
        "cloudflared" => start_cloudflared(port),
        "ngrok" => start_ngrok(port),
        "none" => Err(anyhow!("Tunnel provider is 'none'")),
        _ => Err(anyhow!("Unknown tunnel provider: {}", provider)),
    }
}

fn start_cloudflared(port: u16) -> Result<TunnelHandle> {
    info!("Starting cloudflared tunnel on port {}", port);

    let mut child = Command::new("cloudflared")
        .args(&["tunnel", "--url", &format!("http://localhost:{}", port)])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped()) // cloudflared often outputs to stderr
        .spawn()
        .context("Failed to spawn cloudflared. Is it installed?")?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("Failed to capture stderr"))?;
    let reader = BufReader::new(stderr);

    let (tx, rx) = std::sync::mpsc::channel();

    let re = Regex::new(r"https://[a-zA-Z0-9-]+\.trycloudflare\.com")
        .expect("Invalid regex for cloudflare tunnel");
    thread::spawn(move || {
        for l in reader.lines().flatten() {
            if let Some(mat) = re.find(&l) {
                let _ = tx.send(mat.as_str().to_string());
                break;
            }
        }
    });

    // Wait for URL with timeout
    match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(url) => {
            info!("Cloudflared tunnel active at: {}", url);
            Ok(TunnelHandle {
                info: TunnelInfo {
                    provider: "cloudflared".to_string(),
                    public_url: url,
                    local_port: port,
                },
                child: Arc::new(Mutex::new(child)),
            })
        }
        Err(_) => {
            let _ = child.kill();
            Err(anyhow!("Timed out waiting for cloudflared URL"))
        }
    }
}

fn start_ngrok(port: u16) -> Result<TunnelHandle> {
    info!("Starting ngrok tunnel on port {}", port);

    // ngrok http 8080 --log=stdout --log-format=json
    let mut child = Command::new("ngrok")
        .args(&[
            "http",
            &port.to_string(),
            "--log=stdout",
            "--log-format=json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn ngrok. Is it installed?")?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Failed to capture stdout"))?;
    let reader = BufReader::new(stdout);

    let (tx, rx) = std::sync::mpsc::channel();

    thread::spawn(move || {
        // Look for JSON with "url" field
        for l in reader.lines().flatten() {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&l) {
                if let Some(url) = json.get("url").and_then(|v| v.as_str()) {
                    let _ = tx.send(url.to_string());
                    break;
                }
            }
        }
    });

    match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(url) => {
            info!("Ngrok tunnel active at: {}", url);
            Ok(TunnelHandle {
                info: TunnelInfo {
                    provider: "ngrok".to_string(),
                    public_url: url,
                    local_port: port,
                },
                child: Arc::new(Mutex::new(child)),
            })
        }
        Err(_) => {
            let _ = child.kill();
            Err(anyhow!("Timed out waiting for ngrok URL"))
        }
    }
}
