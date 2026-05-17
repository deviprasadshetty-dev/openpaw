//! Google Meet tool — join a Meet call, scrape live captions, produce a transcript.
//!
//! Uses the existing CDP client (Chrome DevTools Protocol) to control a headless
//! Chromium instance. The caption-scraping strategy mirrors hermes-agent's
//! OpenUtter approach: we enable Google Meet's built-in live captions and observe
//! the captions container in the DOM via a MutationObserver.
//!
//! ## Actions
//!
//! | Action          | Description |
//! |-----------------|-------------|
//! | `meet_join`     | Join a meet.google.com URL, start caption scraping |
//! | `meet_status`   | Report bot liveness + transcript progress |
//! | `meet_transcript` | Read the current transcript (optional last-N lines) |
//! | `meet_leave`    | Leave the call cleanly, stop scraping, finalize transcript |
//! | `meet_say`      | (stub) Speak text into the meeting via audio bridge |
//!
//! ## Safety
//!
//! - URL gate: only `https://meet.google.com/...` URLs pass.
//! - No calendar scanning, no auto-dial, no auto-consent announcement.
//! - One active meeting at a time. A second `meet_join` leaves the first.

use super::cdp::{self, CdpClient};
use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

// ── Caption observer JavaScript ─────────────────────────────────────────
// Same strategy as hermes-agent: inject a MutationObserver on the caption
// container, expose `window.__openpawMeetDrain()` to pull new entries.

const CAPTION_OBSERVER_JS: &str = r#"
(() => {
  if (window.__openpawMeetInstalled) return;
  window.__openpawMeetInstalled = true;
  window.__openpawMeetQueue = [];

  const captionSelector = '[role="region"][aria-label*="aption" i], ' +
                          'div[jsname="YSxPC"], ' +  // legacy
                          'div[jsname="tgaKEf"]';    // current

  function pushEntry(speaker, text) {
    if (!text || !text.trim()) return;
    window.__openpawMeetQueue.push({
      ts: Date.now(),
      speaker: (speaker || '').trim(),
      text: text.trim(),
    });
  }

  function scan(root) {
    const rows = root.querySelectorAll('div[jsname="dsyhDe"], div.CNusmb, div.TBMuR');
    if (rows.length) {
      rows.forEach((row) => {
        const spkEl = row.querySelector('div.KcIKyf, div.zs7s8d, span[jsname="YSxPC"]');
        const txtEl = row.querySelector('div.bh44bd, span[jsname="tgaKEf"], div.iTTPOb');
        const speaker = spkEl ? spkEl.innerText : '';
        const text = txtEl ? txtEl.innerText : row.innerText;
        pushEntry(speaker, text);
      });
      return;
    }
    const text = (root.innerText || '').split('\n').filter(Boolean).pop();
    pushEntry('', text);
  }

  function attach() {
    const el = document.querySelector(captionSelector);
    if (!el) return false;
    const obs = new MutationObserver(() => scan(el));
    obs.observe(el, { childList: true, subtree: true, characterData: true });
    scan(el);
    return true;
  }

  if (!attach()) {
    const iv = setInterval(() => { if (attach()) clearInterval(iv); }, 1500);
  }

  window.__openpawMeetDrain = () => {
    const out = window.__openpawMeetQueue.slice();
    window.__openpawMeetQueue = [];
    return out;
  };
})();
"#;

const ENABLE_CAPTIONS_JS: &str = r#"
(() => {
  const ev = new KeyboardEvent('keydown', {
    key: 'c', code: 'KeyC', keyCode: 67, which: 67, bubbles: true,
  });
  document.body.dispatchEvent(ev);
  return true;
})();
"#;

const ADMISSION_PROBE_JS: &str = r#"
(() => {
  const leave = document.querySelector('button[aria-label*="eave call" i]');
  if (leave) return true;
  if (window.__openpawMeetInstalled) {
    const caps = document.querySelector(
      '[role="region"][aria-label*="aption" i], ' +
      'div[jsname="YSxPC"], div[jsname="tgaKEf"]'
    );
    if (caps) return true;
  }
  const parts = document.querySelector('[aria-label*="articipants" i]');
  if (parts) return true;
  return false;
})();
"#;

const DENIED_PROBE_JS: &str = r#"
(() => {
  const text = document.body ? document.body.innerText || '' : '';
  if (/You can't join this video call/i.test(text)) return true;
  if (/You were removed from the meeting/i.test(text)) return true;
  if (/No one responded to your request to join/i.test(text)) return true;
  return false;
})();
"#;

const LEAVE_BUTTON_JS: &str = r#"
(() => {
  const b = document.querySelector('button[aria-label*="eave call" i]');
  if (b) b.click();
})();
"#;

// ── Meet state ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct MeetSession {
    meeting_id: String,
    url: String,
    out_dir: String,
    transcript_path: String,
    status_path: String,
    in_call: bool,
    captioning: bool,
    join_attempted_at: Option<f64>,
    joined_at: Option<f64>,
    last_caption_at: Option<f64>,
    transcript_lines: usize,
    lobby_waiting: bool,
    leave_reason: Option<String>,
    error: Option<String>,
    mode: String,
}

impl MeetSession {
    fn status_json(&self) -> Value {
        json!({
            "ok": true,
            "alive": true,
            "meetingId": self.meeting_id,
            "url": self.url,
            "outDir": self.out_dir,
            "inCall": self.in_call,
            "captioning": self.captioning,
            "lobbyWaiting": self.lobby_waiting,
            "joinAttemptedAt": self.join_attempted_at,
            "joinedAt": self.joined_at,
            "lastCaptionAt": self.last_caption_at,
            "transcriptLines": self.transcript_lines,
            "transcriptPath": self.transcript_path,
            "leaveReason": self.leave_reason,
            "error": self.error,
            "mode": self.mode,
        })
    }
}

// ── MeetTool ────────────────────────────────────────────────────────────

pub struct MeetTool {
    workspace_dir: String,
    cdp_host: String,
    cdp_port: u16,
    headless: bool,
    browser_path: Option<String>,
    profile_dir: Option<String>,
    auto_launch: bool,
    /// Active meeting session state, shared with the background polling task.
    session: Arc<Mutex<Option<MeetSession>>>,
    /// CDP client for the meet-dedicated browser.
    client: Arc<Mutex<Option<CdpClient>>>,
    /// Browser process handle.
    browser_process: Arc<Mutex<Option<std::process::Child>>>,
}

impl MeetTool {
    pub fn new(
        workspace_dir: impl Into<String>,
        config: &crate::config_types::BrowserConfig,
    ) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
            cdp_host: config.cdp_host.clone(),
            cdp_port: config.cdp_port,
            headless: config.native_headless,
            browser_path: config.native_chrome_path.clone(),
            profile_dir: config.profile_dir.clone(),
            auto_launch: config.cdp_auto_launch,
            session: Arc::new(Mutex::new(None)),
            client: Arc::new(Mutex::new(None)),
            browser_process: Arc::new(Mutex::new(None)),
        }
    }

    fn meetings_dir(&self) -> std::path::PathBuf {
        Path::new(&self.workspace_dir).join("meetings")
    }

    /// Get or connect a CDP client for this tool. Uses a separate debug port
    /// from the main browser tool — we offset by 1 to avoid collisions.
    async fn get_client(&self) -> Result<CdpClient> {
        let mut guard = self.client.lock().await;
        if let Some(ref c) = *guard {
            return Ok(c.clone());
        }

        // Use an offset port to avoid colliding with the main browser tool.
        let port = self.cdp_port + 1;
        let endpoint = format!("{}:{}", self.cdp_host, port);
        let cdp = CdpClient::new(&endpoint);

        if let Err(e) = cdp.connect().await {
            if self.auto_launch {
                tracing::info!(
                    "Meet: browser not reachable on {}, auto-launching",
                    endpoint
                );
                let mut proc_guard = self.browser_process.lock().await;
                let child = cdp::launch_browser(
                    port,
                    self.headless,
                    self.browser_path.as_deref(),
                    &self.workspace_dir,
                    self.profile_dir.as_deref(),
                )
                .await?;
                *proc_guard = Some(child);
                drop(proc_guard);

                tokio::time::sleep(tokio::time::Duration::from_millis(2500)).await;

                for attempt in 0..5 {
                    match cdp.connect().await {
                        Ok(()) => break,
                        Err(conn_err) => {
                            if attempt == 4 {
                                return Err(conn_err);
                            }
                            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                        }
                    }
                }
            } else {
                return Err(e);
            }
        }

        cdp.enable_page_domain().await?;
        cdp.enable_network().await?;

        *guard = Some(cdp.clone());
        Ok(cdp)
    }

    fn is_safe_meet_url(url: &str) -> bool {
        let url = url.trim();
        if !url.starts_with("https://meet.google.com/") {
            return false;
        }
        // Must have a meeting code (3 segments separated by dashes) or /new or /lookup/
        let re = regex::Regex::new(
            r"^https://meet\.google\.com/([a-z0-9]{3,}-[a-z0-9]{3,}-[a-z0-9]{3,}|lookup/[^/?#]+|new)([/?#].*)?$",
        )
        .unwrap();
        re.is_match(url)
    }

    /// Extract the meeting id from a URL for filenames.
    fn meeting_id_from_url(url: &str) -> String {
        let re = regex::Regex::new(
            r"meet\.google\.com/([a-z0-9]{3,}-[a-z0-9]{3,}-[a-z0-9]{3,})",
        )
        .unwrap();
        if let Some(caps) = re.captures(url) {
            caps[1].to_string()
        } else {
            format!(
                "meet-{}",
                chrono::Utc::now().format("%Y%m%d_%H%M%S")
            )
        }
    }

    fn parse_duration(raw: &str) -> Option<f64> {
        let raw = raw.trim().to_lowercase();
        if raw.is_empty() {
            return None;
        }
        if let Some(s) = raw.strip_suffix('h') {
            return s.parse::<f64>().ok().map(|v| v * 3600.0);
        }
        if let Some(s) = raw.strip_suffix('m') {
            return s.parse::<f64>().ok().map(|v| v * 60.0);
        }
        if let Some(s) = raw.strip_suffix('s') {
            return s.parse::<f64>().ok();
        }
        raw.parse::<f64>().ok()
    }
}

impl Clone for MeetTool {
    fn clone(&self) -> Self {
        Self {
            workspace_dir: self.workspace_dir.clone(),
            cdp_host: self.cdp_host.clone(),
            cdp_port: self.cdp_port,
            headless: self.headless,
            browser_path: self.browser_path.clone(),
            profile_dir: self.profile_dir.clone(),
            auto_launch: self.auto_launch,
            session: self.session.clone(),
            client: self.client.clone(),
            browser_process: self.browser_process.clone(),
        }
    }
}

unsafe impl Send for MeetTool {}
unsafe impl Sync for MeetTool {}

// ── Background polling task ──────────────────────────────────────────────

/// Spawns a background task that runs the caption-scraping loop for the
/// active meeting. The task updates `session` state in-place and appends
/// captions to `transcript.txt`. Runs until the stop flag is set, the
/// meeting ends, or an error occurs.
async fn run_meet_poll_loop(
    cdp: CdpClient,
    session_ref: Arc<Mutex<Option<MeetSession>>>,
    url: String,
    guest_name: String,
    duration_secs: Option<f64>,
    lobby_timeout_secs: f64,
) {
    // Navigate to the meet URL
    if let Err(e) = cdp.navigate(&url).await {
        let mut s = session_ref.lock().await;
        if let Some(ref mut sess) = *s {
            sess.error = Some(format!("navigate failed: {}", e));
        }
        return;
    }

    // Wait for page to load
    let _ = cdp.wait_for_load("load").await;
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Try to fill guest name input
    let fill_guest_js = format!(
        r#"
        (() => {{
            const inp = document.querySelector('input[aria-label*="name" i]');
            if (inp) {{
                inp.value = {};
                inp.dispatchEvent(new Event('input', {{bubbles: true}}));
                inp.dispatchEvent(new Event('change', {{bubbles: true}}));
                return true;
            }}
            return false;
        }})()
        "#,
        serde_json::to_string(&guest_name).unwrap_or_default()
    );
    let _ = cdp.evaluate(&fill_guest_js, false).await;

    // Click "Join now" or "Ask to join"
    let click_join_js = r#"
    (() => {
      const btns = document.querySelectorAll('button');
      for (const btn of btns) {
        if (/Ask to join|Join now/i.test(btn.innerText || '')) {
          btn.click();
          return true;
        }
      }
      return false;
    })();
    "#;
    if let Ok(res) = cdp.evaluate(click_join_js, false).await {
        let was_lobby = res
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if was_lobby {
            let mut s = session_ref.lock().await;
            if let Some(ref mut sess) = *s {
                sess.lobby_waiting = true;
            }
        }
    }

    // Enable captions
    let _ = cdp.evaluate(ENABLE_CAPTIONS_JS, false).await;

    // Install caption observer
    if let Err(e) = cdp.evaluate(CAPTION_OBSERVER_JS, false).await {
        let mut s = session_ref.lock().await;
        if let Some(ref mut sess) = *s {
            sess.error = Some(format!("caption observer install failed: {}", e));
        }
        return;
    }

    // Mark captioning as active
    {
        let mut s = session_ref.lock().await;
        if let Some(ref mut sess) = *s {
            sess.captioning = true;
            sess.join_attempted_at = Some(
                chrono::Utc::now().timestamp_millis() as f64 / 1000.0,
            );
        }
    }

    let start = tokio::time::Instant::now();
    let deadline = duration_secs.map(|d| start + tokio::time::Duration::from_secs_f64(d));
    let lobby_deadline = start + tokio::time::Duration::from_secs_f64(lobby_timeout_secs);
    let mut last_admission_check = tokio::time::Instant::now();
    let poll_interval = tokio::time::Duration::from_secs(1);

    loop {
        tokio::time::sleep(poll_interval).await;

        // Check if we've been told to stop
        {
            let s = session_ref.lock().await;
            if s.is_none() {
                // Session was cleared by meet_leave
                break;
            }
        }

        // Check duration expiry
        if let Some(dl) = deadline {
            if tokio::time::Instant::now() >= dl {
                let mut s = session_ref.lock().await;
                if let Some(ref mut sess) = *s {
                    sess.leave_reason = Some("duration_expired".to_string());
                }
                break;
            }
        }

        // Check if page was closed
        if let Ok(url) = cdp.get_current_url().await {
            if url.is_empty() || url == "about:blank" {
                let mut s = session_ref.lock().await;
                if let Some(ref mut sess) = *s {
                    sess.leave_reason = Some("page_closed".to_string());
                }
                break;
            }
        }

        // Admission detection (~every 3s until admitted)
        {
            let need_check = {
                let s = session_ref.lock().await;
                s.as_ref().map(|sess| !sess.in_call).unwrap_or(false)
            };
            if need_check && last_admission_check.elapsed() >= tokio::time::Duration::from_secs(3) {
                last_admission_check = tokio::time::Instant::now();

                // Check if admitted
                if let Ok(result) = cdp.evaluate(ADMISSION_PROBE_JS, false).await {
                    let admitted = result
                        .get("result")
                        .and_then(|r| r.get("value"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if admitted {
                        let mut s = session_ref.lock().await;
                        if let Some(ref mut sess) = *s {
                            sess.in_call = true;
                            sess.lobby_waiting = false;
                            sess.joined_at = Some(
                                chrono::Utc::now().timestamp_millis() as f64 / 1000.0,
                            );
                        }
                    } else if tokio::time::Instant::now() >= lobby_deadline {
                        let mut s = session_ref.lock().await;
                        if let Some(ref mut sess) = *s {
                            sess.error = Some(
                                "lobby timeout — host never admitted the bot".to_string(),
                            );
                            sess.leave_reason = Some("lobby_timeout".to_string());
                        }
                        break;
                    }
                }

                // Check if denied
                if let Ok(result) = cdp.evaluate(DENIED_PROBE_JS, false).await {
                    let denied = result
                        .get("result")
                        .and_then(|r| r.get("value"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if denied {
                        let mut s = session_ref.lock().await;
                        if let Some(ref mut sess) = *s {
                            sess.error = Some("host denied admission".to_string());
                            sess.leave_reason = Some("denied".to_string());
                        }
                        break;
                    }
                }
            }
        }

        // Drain captions
        if let Ok(result) = cdp.evaluate("window.__openpawMeetDrain && window.__openpawMeetDrain()", false).await {
            if let Some(entries) = result
                .get("result")
                .and_then(|r| r.get("value"))
                .and_then(|v| v.as_array())
            {
                for entry in entries {
                    let speaker = entry
                        .get("speaker")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown")
                        .trim()
                        .to_string();
                    let text = entry
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();

                    if text.is_empty() {
                        continue;
                    }

                    let (transcript_path, _meeting_id) = {
                        let s = session_ref.lock().await;
                        s.as_ref().map(|sess| {
                            (sess.transcript_path.clone(), sess.meeting_id.clone())
                        })
                        .unwrap_or_default()
                    };

                    if transcript_path.is_empty() {
                        continue;
                    }

                    // Dedup: we use a simple approach — append each line.
                    // Full dedup would need a more complex state but for a
                    // single meeting this is good enough.
                    let now = chrono::Utc::now();
                    let ts = now.format("%H:%M:%S");
                    let speaker_display = if speaker.is_empty() || speaker == "Unknown" {
                        "Unknown".to_string()
                    } else {
                        speaker.clone()
                    };
                    let line = format!("[{}] {}: {}\n", ts, speaker_display, text);

                    // Atomic-ish append
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&transcript_path)
                    {
                        use std::io::Write;
                        let _ = file.write_all(line.as_bytes());
                    }

                    let mut s = session_ref.lock().await;
                    if let Some(ref mut sess) = *s {
                        sess.transcript_lines += 1;
                        sess.last_caption_at =
                            Some(now.timestamp_millis() as f64 / 1000.0);
                    }
                }
            }
        }

        // Flush status.json periodically
        {
            let s = session_ref.lock().await;
            if let Some(ref sess) = *s {
                let status = sess.status_json();
                if let Ok(json_str) = serde_json::to_string_pretty(&status) {
                    let tmp = format!("{}.tmp", sess.status_path);
                    let _ = std::fs::write(&tmp, &json_str);
                    let _ = std::fs::rename(&tmp, &sess.status_path);
                }
            }
        }
    }

    // Click "Leave call" button if present
    let _ = cdp.evaluate(LEAVE_BUTTON_JS, false).await;
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Final status update
    {
        let mut s = session_ref.lock().await;
        if let Some(ref mut sess) = *s {
            sess.in_call = false;
            sess.captioning = false;
            let status = sess.status_json();
            if let Ok(json_str) = serde_json::to_string_pretty(&status) {
                let tmp = format!("{}.tmp", sess.status_path);
                let _ = std::fs::write(&tmp, &json_str);
                let _ = std::fs::rename(&tmp, &sess.status_path);
            }
        }
    }

    let _ = cdp.disconnect().await;
}

// ── Tool trait implementation ────────────────────────────────────────────

#[async_trait]
impl Tool for MeetTool {
    fn name(&self) -> &str {
        "google_meet"
    }

    fn description(&self) -> &str {
        "Join a Google Meet call, scrape live captions into a transcript, and follow up. \
         Actions: meet_join (start bot), meet_status (liveness + progress), \
         meet_transcript (read captions), meet_leave (stop bot), meet_say (speak in call). \
         Only meet.google.com URLs are accepted. No calendar scanning, no auto-dial. \
         Flow: meet_join → poll with meet_status/meet_transcript → meet_leave when done."
    }

    fn parameters_json(&self) -> String {
        r#"{
          "type": "object",
          "properties": {
            "action": {
              "type": "string",
              "enum": ["meet_join", "meet_status", "meet_transcript", "meet_leave", "meet_say"],
              "description": "Action to perform"
            },
            "url": {
              "type": "string",
              "description": "Full https://meet.google.com/... URL for meet_join. Required."
            },
            "mode": {
              "type": "string",
              "enum": ["transcribe"],
              "description": "Mode: transcribe (default) — listen-only, scrape captions."
            },
            "guest_name": {
              "type": "string",
              "description": "Display name when joining as guest. Default: 'Hermes Agent'."
            },
            "duration": {
              "type": "string",
              "description": "Max duration before auto-leave (e.g. '30m', '2h', '90s'). Omit to stay until meet_leave."
            },
            "last": {
              "type": "integer",
              "description": "For meet_transcript: return only the last N caption lines (optional).",
              "minimum": 1
            },
            "text": {
              "type": "string",
              "description": "Text to speak into the call (meet_say, future realtime mode)."
            }
          },
          "required": ["action"]
        }"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");

        match action {
            "meet_join" => self.handle_meet_join(&args).await,
            "meet_status" => self.handle_meet_status(&args).await,
            "meet_transcript" => self.handle_meet_transcript(&args).await,
            "meet_leave" => self.handle_meet_leave(&args).await,
            "meet_say" => self.handle_meet_say(&args).await,
            _ => Ok(ToolResult::fail(format!(
                "Unknown action '{}'. Available: meet_join, meet_status, meet_transcript, meet_leave, meet_say",
                action
            ))),
        }
    }
}

// ── Action handlers ──────────────────────────────────────────────────────

impl MeetTool {
    async fn handle_meet_join(&self, args: &Value) -> Result<ToolResult> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if url.is_empty() {
            return Ok(ToolResult::fail("url is required for meet_join"));
        }

        if !Self::is_safe_meet_url(&url) {
            return Ok(ToolResult::fail(format!(
                "refusing: only https://meet.google.com/ URLs are allowed. got: {}",
                url
            )));
        }

        // If a meeting is already active, leave it first
        {
            let has_session = {
                let s = self.session.lock().await;
                s.is_some()
            };
            if has_session {
                let _ = self.handle_meet_leave(args).await;
            }
        }

        let guest_name = args
            .get("guest_name")
            .and_then(|v| v.as_str())
            .unwrap_or("Hermes Agent")
            .to_string();

        let duration_raw = args
            .get("duration")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let duration_secs = Self::parse_duration(duration_raw);
        let lobby_timeout = 300.0; // 5 minutes

        // Connect CDP client
        let cdp = match self.get_client().await {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult::fail(format!(
                    "CDP connection error: {}",
                    e
                )));
            }
        };

        let meeting_id = Self::meeting_id_from_url(&url);
        let out_dir = self
            .meetings_dir()
            .join(&meeting_id)
            .to_string_lossy()
            .to_string();

        // Ensure output directory exists
        let _ = std::fs::create_dir_all(&out_dir);

        let transcript_path = Path::new(&out_dir).join("transcript.txt");
        let status_path = Path::new(&out_dir).join("status.json");

        // Clear stale files
        let _ = std::fs::remove_file(&transcript_path);
        let _ = std::fs::remove_file(&status_path);

        let mode = args
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("transcribe")
            .to_string();

        let session = MeetSession {
            meeting_id: meeting_id.clone(),
            url: url.clone(),
            out_dir: out_dir.clone(),
            transcript_path: transcript_path.to_string_lossy().to_string(),
            status_path: status_path.to_string_lossy().to_string(),
            in_call: false,
            captioning: false,
            join_attempted_at: None,
            joined_at: None,
            last_caption_at: None,
            transcript_lines: 0,
            lobby_waiting: false,
            leave_reason: None,
            error: None,
            mode: mode.clone(),
        };

        // Write initial status
        let status = session.status_json();
        if let Ok(json_str) = serde_json::to_string_pretty(&status) {
            let _ = std::fs::write(&status_path, &json_str);
        }

        // Store session
        {
            let mut s = self.session.lock().await;
            *s = Some(session);
        }

        // Spawn background polling task
        let cdp_for_task = cdp.clone();
        let session_ref = self.session.clone();

        tokio::spawn(async move {
            run_meet_poll_loop(
                cdp_for_task,
                session_ref,
                url,
                guest_name,
                duration_secs,
                lobby_timeout,
            )
            .await;
        });

        Ok(ToolResult::ok(
            serde_json::to_string_pretty(&json!({
                "success": true,
                "meetingId": meeting_id,
                "outDir": out_dir,
                "mode": mode,
                "message": "Bot is joining the meeting. Poll with meet_status and meet_transcript."
            }))
            .unwrap_or_default(),
        ))
    }

    async fn handle_meet_status(&self, _args: &Value) -> Result<ToolResult> {
        let s = self.session.lock().await;
        match *s {
            Some(ref session) => {
                // Re-read status from file to get latest state from background task
                let status_json = if Path::new(&session.status_path).exists() {
                    std::fs::read_to_string(&session.status_path)
                        .ok()
                        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                        .unwrap_or(session.status_json())
                } else {
                    session.status_json()
                };

                Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&status_json).unwrap_or_default(),
                ))
            }
            None => Ok(ToolResult::ok(
                serde_json::to_string_pretty(&json!({
                    "ok": false,
                    "reason": "no active meeting"
                }))
                .unwrap_or_default(),
            )),
        }
    }

    async fn handle_meet_transcript(&self, args: &Value) -> Result<ToolResult> {
        let last = args.get("last").and_then(|v| v.as_u64()).map(|n| n as usize);

        let s = self.session.lock().await;
        match *s {
            Some(ref session) => {
                let tp = Path::new(&session.transcript_path);
                if !tp.exists() {
                    return Ok(ToolResult::ok(
                        serde_json::to_string_pretty(&json!({
                            "ok": true,
                            "meetingId": session.meeting_id,
                            "lines": [],
                            "total": 0,
                            "path": session.transcript_path,
                        }))
                        .unwrap_or_default(),
                    ));
                }

                let text = match std::fs::read_to_string(tp) {
                    Ok(t) => t,
                    Err(e) => {
                        return Ok(ToolResult::fail(format!(
                            "Failed to read transcript: {}",
                            e
                        )));
                    }
                };

                let all_lines: Vec<&str> =
                    text.lines().filter(|l| !l.trim().is_empty()).collect();
                let total = all_lines.len();

                let lines: Vec<&str> = match last {
                    Some(n) if n < total => all_lines[total - n..].to_vec(),
                    _ => all_lines.clone(),
                };

                Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&json!({
                        "ok": true,
                        "meetingId": session.meeting_id,
                        "lines": lines,
                        "total": total,
                        "path": session.transcript_path,
                    }))
                    .unwrap_or_default(),
                ))
            }
            None => Ok(ToolResult::ok(
                serde_json::to_string_pretty(&json!({
                    "ok": false,
                    "reason": "no active meeting"
                }))
                .unwrap_or_default(),
            )),
        }
    }

    async fn handle_meet_leave(&self, _args: &Value) -> Result<ToolResult> {
        let meeting_id;
        let transcript_path;

        {
            let mut s = self.session.lock().await;
            match *s {
                Some(ref session) => {
                    meeting_id = session.meeting_id.clone();
                    transcript_path = session.transcript_path.clone();
                }
                None => {
                    return Ok(ToolResult::ok(
                        serde_json::to_string_pretty(&json!({
                            "ok": false,
                            "reason": "no active meeting"
                        }))
                        .unwrap_or_default(),
                    ));
                }
            }

            // Update leave reason before clearing
            if let Some(ref mut session) = *s {
                session.leave_reason = Some("agent called meet_leave".to_string());
            }

            // Clear the session — this signals the background task to stop
            *s = None;
        }

        // Disconnect CDP (background task may also be doing this)
        {
            let mut guard = self.client.lock().await;
            if let Some(cdp) = guard.take() {
                let _ = cdp.disconnect().await;
            }
        }

        // Kill browser process
        {
            let mut proc_guard = self.browser_process.lock().await;
            if let Some(mut child) = proc_guard.take() {
                tracing::info!(
                    "Meet: killing browser process (pid {})",
                    child.id()
                );
                let _ = child.kill();
                let _ = child.wait();
            }
        }

        Ok(ToolResult::ok(
            serde_json::to_string_pretty(&json!({
                "ok": true,
                "reason": "agent called meet_leave",
                "meetingId": meeting_id,
                "transcriptPath": transcript_path,
            }))
            .unwrap_or_default(),
        ))
    }

    async fn handle_meet_say(&self, args: &Value) -> Result<ToolResult> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if text.is_empty() {
            return Ok(ToolResult::fail("text is required for meet_say"));
        }

        // Realtime mode is not yet implemented.
        // This is a stub — in the future, this would queue text to an
        // OpenAI Realtime session and stream TTS audio into the call.
        let s = self.session.lock().await;
        match *s {
            Some(ref session) => {
                if session.mode == "realtime" {
                    Ok(ToolResult::fail(
                        "realtime mode (meet_say) is not yet implemented. \
                         The bot can only transcribe captions at this time.",
                    ))
                } else {
                    Ok(ToolResult::fail(
                        "active meeting is in transcribe mode — pass mode='realtime' \
                         to meet_join to enable agent speech (not yet supported).",
                    ))
                }
            }
            None => Ok(ToolResult::fail("no active meeting — use meet_join first")),
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_safe_meet_url() {
        // Valid URLs
        assert!(MeetTool::is_safe_meet_url(
            "https://meet.google.com/abc-defg-hij"
        ));
        assert!(MeetTool::is_safe_meet_url(
            "https://meet.google.com/abc-defg-hij?authuser=1"
        ));
        assert!(MeetTool::is_safe_meet_url(
            "https://meet.google.com/new"
        ));
        assert!(MeetTool::is_safe_meet_url(
            "https://meet.google.com/lookup/something"
        ));

        // Invalid URLs
        assert!(!MeetTool::is_safe_meet_url(""));
        assert!(!MeetTool::is_safe_meet_url("not a url"));
        assert!(!MeetTool::is_safe_meet_url(
            "http://meet.google.com/abc-defg-hij"
        ));
        assert!(!MeetTool::is_safe_meet_url(
            "https://zoom.us/j/123456"
        ));
        assert!(!MeetTool::is_safe_meet_url(
            "https://meet.google.com/"
        ));
    }

    #[test]
    fn test_parse_duration() {
        assert_eq!(MeetTool::parse_duration("30m"), Some(1800.0));
        assert_eq!(MeetTool::parse_duration("2h"), Some(7200.0));
        assert_eq!(MeetTool::parse_duration("90s"), Some(90.0));
        assert_eq!(MeetTool::parse_duration("90"), Some(90.0));
        assert_eq!(MeetTool::parse_duration(""), None);
        assert_eq!(MeetTool::parse_duration("1.5h"), Some(5400.0));
    }

    #[test]
    fn test_meeting_id_from_url() {
        let id = MeetTool::meeting_id_from_url(
            "https://meet.google.com/abc-defg-hij",
        );
        assert_eq!(id, "abc-defg-hij");

        let id = MeetTool::meeting_id_from_url(
            "https://meet.google.com/abc-defg-hij?authuser=0",
        );
        assert_eq!(id, "abc-defg-hij");
    }
}
