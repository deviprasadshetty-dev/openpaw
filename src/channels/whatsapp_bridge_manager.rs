use anyhow::{Result, anyhow};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use tracing::{info, warn};

pub struct BridgeManager {
    child: Arc<Mutex<Option<Child>>>,
    bridge_dir: String,
}

impl BridgeManager {
    pub fn new(bridge_dir: String) -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            bridge_dir,
        }
    }

    pub fn start(&self) -> Result<()> {
        let mut guard = self.child.lock().unwrap();
        if guard.is_some() {
            return Ok(());
        }

        info!("Starting WhatsApp Native Bridge in {}...", self.bridge_dir);

        // Check if go is installed
        let go_check = Command::new("go")
            .arg("version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        if go_check.is_err() || !go_check.unwrap().success() {
            return Err(anyhow!(
                "Go is not installed or not in PATH. Please install Go to use the WhatsApp Bridge automatically."
            ));
        }

        // Run go mod tidy first to ensure dependencies are resolved
        let _ = Command::new("go")
            .arg("mod")
            .arg("tidy")
            .current_dir(&self.bridge_dir)
            .status();

        let mut child = Command::new("go")
            .arg("run")
            .arg("main.go")
            .current_dir(&self.bridge_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        // Spawn threads to capture output
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for l in reader.lines().map_while(Result::ok) {
                info!("[WhatsApp Bridge] {}", l);
                // Special detection for QR codes or pairing status could go here
            }
        });

        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for l in reader.lines().map_while(Result::ok) {
                warn!("[WhatsApp Bridge Error] {}", l);
            }
        });

        *guard = Some(child);
        Ok(())
    }

    pub fn stop(&self) {
        let mut guard = self.child.lock().unwrap();
        if let Some(mut child) = guard.take() {
            info!("Stopping WhatsApp Native Bridge...");
            let _ = child.kill();
        }
    }

    pub fn is_running(&self) -> bool {
        let mut guard = self.child.lock().unwrap();
        if let Some(ref mut child) = *guard {
            match child.try_wait() {
                Ok(None) => true,
                _ => {
                    *guard = None;
                    false
                }
            }
        } else {
            false
        }
    }
}

impl Drop for BridgeManager {
    fn drop(&mut self) {
        self.stop();
    }
}
