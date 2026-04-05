use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use tokio::process::Command;

pub struct ScreenshotTool {
    pub workspace_dir: String,
}

#[async_trait]
impl Tool for ScreenshotTool {
    fn name(&self) -> &str {
        "screenshot"
    }

    fn description(&self) -> &str {
        "Take a screenshot of the primary display and save it to the workspace. Returns the file path."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"filename":{"type":"string","description":"Output filename (optional, defaults to screenshot_<timestamp>.png)"}}}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let filename = args
            .get("filename")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("screenshot_{}.png", ts));

        // Sanitise: strip path separators so the file always lands in workspace_dir
        let safe_filename = filename
            .replace(['/', '\\', '\0'], "_")
            .trim_start_matches('.')
            .to_string();

        let out_path = PathBuf::from(&self.workspace_dir).join(&safe_filename);
        let out_str = out_path.to_string_lossy().to_string();

        let result = take_screenshot(&out_str).await;

        match result {
            Ok(_) if out_path.exists() => Ok(ToolResult::ok(format!(
                "Screenshot saved to: {}",
                out_str
            ))),
            Ok(msg) => Ok(ToolResult::fail(format!(
                "Screenshot command ran but file not found: {}\n{}",
                out_str, msg
            ))),
            Err(e) => Ok(ToolResult::fail(format!("Screenshot failed: {}", e))),
        }
    }
}

async fn take_screenshot(out_path: &str) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        // screencapture is built-in on every macOS install
        run_cmd("screencapture", &["-x", out_path]).await
    }

    #[cfg(target_os = "linux")]
    {
        // Try candidates in preference order — all zero-install on most desktop distros
        let candidates: &[(&str, &[&str])] = &[
            ("scrot", &[out_path]),
            ("gnome-screenshot", &["-f", out_path]),
            ("import", &["-window", "root", out_path]), // imagemagick
            ("xwd", &["-root", "-silent", "-out", out_path]),
        ];

        for (bin, args) in candidates {
            if which_exists(bin).await {
                return run_cmd(bin, args).await;
            }
        }
        Err("No screenshot utility found. Install scrot, gnome-screenshot, or imagemagick.".to_string())
    }

    #[cfg(target_os = "windows")]
    {
        // Pure PowerShell — no extra installs required
        let ps_script = format!(
            r#"Add-Type -AssemblyName System.Windows.Forms,System.Drawing;
$bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds;
$bmp = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height);
$g = [System.Drawing.Graphics]::FromImage($bmp);
$g.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size);
$g.Dispose();
$bmp.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png);
$bmp.Dispose();"#,
            out_path.replace('\'', "''")
        );
        run_cmd("powershell", &["-NoProfile", "-Command", &ps_script]).await
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err("Screenshot not supported on this platform.".to_string())
    }
}

async fn run_cmd(bin: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("Failed to run {}: {}", bin, e))?;

    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "{} exited with code {}. stderr: {}",
            bin,
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(target_os = "linux")]
async fn which_exists(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}
