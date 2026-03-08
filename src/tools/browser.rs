use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use base64::Engine;
use browser_use::browser::{BrowserSession, config::LaunchOptions};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

// ── Browser detection ────────────────────────────────────────────

/// Finds the first installed Chromium-family browser executable.
/// Order of preference: Chrome → Edge → Brave
/// Override via `OPENPAW_BROWSER` environment variable.
pub fn find_browser() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("OPENPAW_BROWSER") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Some(p);
        }
        tracing::warn!(
            "OPENPAW_BROWSER is set to '{}' but the file does not exist.",
            path
        );
    }

    #[cfg(target_os = "windows")]
    let candidates: &[&str] = &[
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
        r"C:\Program Files (x86)\BraveSoftware\Brave-Browser\Application\brave.exe",
    ];

    #[cfg(target_os = "macos")]
    let candidates: &[&str] = &[
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    ];

    #[cfg(target_os = "linux")]
    let candidates: &[&str] = &[
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium-browser",
        "/usr/bin/chromium",
        "/usr/bin/microsoft-edge",
        "/usr/bin/brave-browser",
    ];

    for path in candidates.iter() {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    // Fallback: check PATH
    for name in &[
        "google-chrome",
        "chromium-browser",
        "chromium",
        "msedge",
        "brave",
    ] {
        if let Ok(p) = which::which(name) {
            return Some(p);
        }
    }

    None
}

/// Canonicalize a path and strip the Windows extended-length prefix `\\?\`.
/// Falls back to the original path on error.
fn strip_unc_prefix(path: &Path) -> String {
    match std::fs::canonicalize(path) {
        Ok(canon) => {
            let s = canon.to_string_lossy();
            // Windows canonicalize produces `\\?\C:\...` — strip the prefix
            if let Some(stripped) = s.strip_prefix(r"\\?\") {
                stripped.to_string()
            } else {
                s.into_owned()
            }
        }
        Err(_) => path.to_string_lossy().to_string(),
    }
}

/// Returns a friendly browser name for display/logging.
fn browser_display_name(path: &Path) -> &'static str {
    let s = path.to_string_lossy().to_lowercase();
    if s.contains("edge") || s.contains("msedge") {
        "Microsoft Edge"
    } else if s.contains("brave") {
        "Brave"
    } else {
        "Google Chrome"
    }
}

// ── Session health-check ─────────────────────────────────────────

/// Returns true if the session's browser process is still reachable.
/// `BrowserSession` has no `is_alive()` method, so we probe by running
/// a trivial JS eval via the underlying tab handle.
fn session_is_alive(s: &BrowserSession) -> bool {
    s.tab().evaluate("1", false).is_ok()
}

// ── Session management ───────────────────────────────────────────

/// Per-instance browser session store.
/// Kept inside the tool struct so multiple `BrowserTool` instances
/// (e.g. parallel agents with different workspace dirs) are fully isolated.
struct SessionStore {
    inner: Mutex<Option<Arc<BrowserSession>>>,
}

impl SessionStore {
    const fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    /// Unlock without panicking on poison — recover the guard instead.
    fn lock(&self) -> MutexGuard<'_, Option<Arc<BrowserSession>>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

// ── BrowserTool ──────────────────────────────────────────────────

pub struct BrowserTool {
    pub workspace_dir: String,
    session_store: SessionStore,
}

impl BrowserTool {
    pub fn new(workspace_dir: impl Into<String>) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
            session_store: SessionStore::new(),
        }
    }

    // ── Internal helpers ─────────────────────────────────────────

    /// Returns a live session, launching the browser if needed.
    /// Discards and relaunches if the existing session has died or if headless mode changes.
    fn session(&self, headless_override: Option<bool>) -> Result<Arc<BrowserSession>> {
        let mut guard = self.session_store.lock();

        let is_headless = headless_override.unwrap_or_else(|| {
            std::env::var("OPENPAW_BROWSER_HEADLESS")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(true) // Default to true for speed
        });

        // Health-check existing session before reusing
        if let Some(ref s) = *guard {
            if session_is_alive(s) {
                // For now, we don't relaunch just for headless change to avoid flickering,
                // but we could if we wanted to be strict.
                return Ok(Arc::clone(s));
            }
            tracing::warn!("Existing browser session is dead — relaunching.");
            *guard = None;
        }

        // --- Launch a new session ---
        let browser_path = find_browser().ok_or_else(|| {
            anyhow::anyhow!(
                "No Chromium-family browser found. \
                 Install Chrome, Edge, or Brave, or set OPENPAW_BROWSER to the executable path."
            )
        })?;

        let browser_name = browser_display_name(&browser_path);

        let profile_path = std::path::Path::new(&self.workspace_dir).join("browser-profile");
        std::fs::create_dir_all(&profile_path)?;
        let profile_dir = strip_unc_prefix(&profile_path);

        // Ensure screenshots directory exists
        let screenshots_path = std::path::Path::new(&self.workspace_dir).join("screenshots");
        std::fs::create_dir_all(&screenshots_path)?;
        let screenshots_dir = strip_unc_prefix(&screenshots_path);

        tracing::info!(
            "Launching {} | profile: {} | screenshots: {}",
            browser_name,
            profile_dir,
            screenshots_dir
        );

        let config = LaunchOptions::new()
            .chrome_path(browser_path)
            .user_data_dir(profile_dir.into())
            .headless(is_headless);

        let session = Arc::new(BrowserSession::launch(config)?);
        *guard = Some(Arc::clone(&session));

        tracing::info!("Browser session started ({})", browser_name);
        Ok(session)
    }

    /// Convert a browser-use `Result<ToolResult, BrowserError>` into our
    /// `anyhow::Result<ToolResult>`. The `E: Display` bound covers both
    /// `BrowserError` and `anyhow::Error` without requiring a concrete type.
    fn map_browser_result(
        res: std::result::Result<browser_use::ToolResult, impl std::fmt::Display>,
        context: &str,
    ) -> Result<ToolResult> {
        match res {
            Err(e) => Ok(ToolResult::fail(format!("{} failed: {}", context, e))),
            Ok(tr) => Ok(if tr.success {
                ToolResult::ok(format!("{:?}", tr.data))
            } else {
                ToolResult::fail(format!("{} error: {:?}", context, tr.error))
            }),
        }
    }

    /// Resolve a `selector` JSON field into browser-use tool args.
    /// Accepts:
    ///   - `"@e5"`  → `{"index": 5}`
    ///   - `"#id"`  → `{"selector": "#id"}`
    ///   - integer  → `{"index": N}`
    fn resolve_selector(
        args: &Value,
    ) -> std::result::Result<serde_json::Map<String, Value>, ToolResult> {
        match args.get("selector") {
            Some(Value::String(s)) => {
                if let Some(rest) = s.strip_prefix("@e") {
                    if let Ok(idx) = rest.parse::<usize>() {
                        let mut m = serde_json::Map::new();
                        m.insert("index".into(), idx.into());
                        return Ok(m);
                    }
                }
                let mut m = serde_json::Map::new();
                m.insert("selector".into(), Value::String(s.clone()));
                Ok(m)
            }
            Some(Value::Number(n)) => {
                let mut m = serde_json::Map::new();
                m.insert("index".into(), Value::Number(n.clone()));
                Ok(m)
            }
            _ => Err(ToolResult::fail("Missing or invalid 'selector'")),
        }
    }

    // ── Actions ──────────────────────────────────────────────────

    /// navigate — go to a URL and wait for the page to settle.
    fn do_navigate(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u.to_owned(),
            None => return Ok(ToolResult::fail("Missing 'url' for navigate")),
        };

        let s = self.session(headless)?;
        s.navigate(&url).map_err(|e| anyhow::anyhow!("{}", e))?;

        // Faster wait logic — only wait if needed
        let _ = s.wait_for_navigation();

        let final_url = s.tab().get_url();
        Ok(ToolResult::ok(format!("Navigated → {}", final_url)))
    }

    /// read_page — get clean Markdown content of the current page.
    fn do_read_page(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(20_000);

        let s = self.session(headless)?;
        let html = s
            .tab()
            .get_content()
            .map_err(|e| anyhow::anyhow!("Failed to get content: {}", e))?;

        // Reuse html_to_text from web_fetch
        let extracted = crate::tools::web_fetch::html_to_text(&html);

        if extracted.len() > max_chars {
            let boundary = crate::tools::web_fetch::floor_char_boundary(&extracted, max_chars);
            let truncated = format!(
                "{}\n\n[Content truncated at {} chars, total {} chars]",
                &extracted[..boundary],
                max_chars,
                extracted.len()
            );
            return Ok(ToolResult::ok(truncated));
        }

        Ok(ToolResult::ok(extracted))
    }

    /// click — click an element by CSS selector or @eN index.
    fn do_click(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let sel = match Self::resolve_selector(args) {
            Ok(s) => s,
            Err(e) => return Ok(e),
        };
        let s = self.session(headless)?;
        Self::map_browser_result(s.execute_tool("click", Value::Object(sel)), "click")
    }

    /// type — focus an element and type text into it.
    /// Set `"append": true` to append instead of replacing existing content.
    fn do_type(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let mut sel = match Self::resolve_selector(args) {
            Ok(s) => s,
            Err(e) => return Ok(e),
        };

        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_owned(),
            None => return Ok(ToolResult::fail("Missing 'text' for type")),
        };

        sel.insert("value".into(), Value::String(text));

        let s = self.session(headless)?;
        // "fill" clears existing content then types; "type" appends key-by-key
        let tool = if args
            .get("append")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            "type"
        } else {
            "fill"
        };
        Self::map_browser_result(s.execute_tool(tool, Value::Object(sel)), "type")
    }

    /// scroll — scroll the page or a scoped element.
    fn do_scroll(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let direction = args
            .get("direction")
            .and_then(|v| v.as_str())
            .unwrap_or("down");

        let amount = args.get("amount").and_then(|v| v.as_i64()).unwrap_or(300);

        let tool_name = match direction {
            "up" => "scroll_up",
            "down" => "scroll_down",
            "left" => "scroll_left",
            "right" => "scroll_right",
            _ => return Ok(ToolResult::fail("'direction' must be up/down/left/right")),
        };

        let mut tool_args = serde_json::Map::new();
        tool_args.insert("amount".into(), amount.into());

        if args.get("selector").is_some() {
            if let Ok(sel) = Self::resolve_selector(args) {
                tool_args.extend(sel);
            }
        }

        let s = self.session(headless)?;
        Self::map_browser_result(
            s.execute_tool(tool_name, Value::Object(tool_args)),
            "scroll",
        )
    }

    /// hover — move the mouse over an element (triggers CSS :hover, dropdowns, tooltips).
    fn do_hover(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let sel = match Self::resolve_selector(args) {
            Ok(s) => s,
            Err(e) => return Ok(e),
        };
        let s = self.session(headless)?;
        Self::map_browser_result(s.execute_tool("hover", Value::Object(sel)), "hover")
    }

    /// select — pick an option from a `<select>` dropdown.
    fn do_select(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let mut sel = match Self::resolve_selector(args) {
            Ok(s) => s,
            Err(e) => return Ok(e),
        };

        let value = match args.get("value").and_then(|v| v.as_str()) {
            Some(v) => v.to_owned(),
            None => return Ok(ToolResult::fail("Missing 'value' for select")),
        };

        sel.insert("value".into(), Value::String(value));

        let s = self.session(headless)?;
        Self::map_browser_result(s.execute_tool("select", Value::Object(sel)), "select")
    }

    /// check / uncheck — toggle a checkbox or radio button.
    fn do_check(&self, args: &Value, check: bool, headless: Option<bool>) -> Result<ToolResult> {
        let mut sel = match Self::resolve_selector(args) {
            Ok(s) => s,
            Err(e) => return Ok(e),
        };
        sel.insert("checked".into(), Value::Bool(check));

        let s = self.session(headless)?;
        Self::map_browser_result(s.execute_tool("check", Value::Object(sel)), "check")
    }

    /// key_press — send a keyboard key or chord (e.g. "Enter", "Escape", "Control+a").
    fn do_key_press(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let key = match args.get("key").and_then(|v| v.as_str()) {
            Some(k) => k.to_owned(),
            None => return Ok(ToolResult::fail("Missing 'key' for key_press")),
        };

        let mut tool_args = serde_json::Map::new();
        tool_args.insert("key".into(), Value::String(key));

        // Optionally scope to a focused element
        if args.get("selector").is_some() {
            if let Ok(sel) = Self::resolve_selector(args) {
                tool_args.extend(sel);
            }
        }

        let s = self.session(headless)?;
        Self::map_browser_result(
            s.execute_tool("key_press", Value::Object(tool_args)),
            "key_press",
        )
    }

    /// wait — pause for a fixed duration OR wait until an element appears.
    fn do_wait(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        // Element-wait takes priority over plain duration
        if args.get("selector").is_some() {
            let mut sel = match Self::resolve_selector(args) {
                Ok(s) => s,
                Err(e) => return Ok(e),
            };
            let timeout_ms = args
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(5000);
            sel.insert("timeout".into(), timeout_ms.into());

            let s = self.session(headless)?;
            return Self::map_browser_result(
                s.execute_tool("wait_for_element", Value::Object(sel)),
                "wait",
            );
        }

        // Plain duration wait, capped at 10 s to avoid accidental hangs
        let ms = args
            .get("ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(1000)
            .min(10_000);
        std::thread::sleep(Duration::from_millis(ms));
        Ok(ToolResult::ok(format!("Waited {}ms", ms)))
    }

    /// get_text — extract the visible text of a specific element.
    fn do_get_text(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let sel = match Self::resolve_selector(args) {
            Ok(s) => s,
            Err(e) => return Ok(e),
        };
        let s = self.session(headless)?;
        Self::map_browser_result(s.execute_tool("get_text", Value::Object(sel)), "get_text")
    }

    /// drag_and_drop — drag from one element/coordinate to another.
    fn do_drag_and_drop(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let source = match args.get("source").and_then(|v| v.as_str()) {
            Some(s) => s.to_owned(),
            None => return Ok(ToolResult::fail("Missing 'source' for drag_and_drop")),
        };
        let target = match args.get("target").and_then(|v| v.as_str()) {
            Some(t) => t.to_owned(),
            None => return Ok(ToolResult::fail("Missing 'target' for drag_and_drop")),
        };

        let tool_args = serde_json::json!({ "source": source, "target": target });

        let s = self.session(headless)?;
        Self::map_browser_result(s.execute_tool("drag_and_drop", tool_args), "drag_and_drop")
    }

    /// back / forward — browser history navigation.
    fn do_history(&self, direction: &str, headless: Option<bool>) -> Result<ToolResult> {
        let s = self.session(headless)?;
        let tool = match direction {
            "back" => "go_back",
            "forward" => "go_forward",
            _ => return Ok(ToolResult::fail("direction must be 'back' or 'forward'")),
        };
        Self::map_browser_result(s.execute_tool(tool, serde_json::json!({})), direction)
    }

    /// new_tab — open a URL in a new browser tab and optionally switch to it.
    fn do_new_tab(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("about:blank")
            .to_owned();
        let switch = args.get("switch").and_then(|v| v.as_bool()).unwrap_or(true);

        let tool_args = serde_json::json!({ "url": url, "switch": switch });
        let s = self.session(headless)?;
        Self::map_browser_result(s.execute_tool("new_tab", tool_args), "new_tab")
    }

    /// switch_tab — bring a specific tab into focus by index or URL fragment.
    fn do_switch_tab(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        if args.get("tab_index").is_none() && args.get("url_contains").is_none() {
            return Ok(ToolResult::fail(
                "Provide 'tab_index' or 'url_contains' for switch_tab",
            ));
        }
        let s = self.session(headless)?;
        Self::map_browser_result(s.execute_tool("switch_tab", args.clone()), "switch_tab")
    }

    /// read_dom — extract the simplified, indexed DOM tree.
    fn do_read_dom(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        // serde_json::Value has no .as_usize() — use .as_u64() and cast
        let max_chars = args
            .get("max_dom_length")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(10_000)
            .clamp(1_000, 100_000);

        let s = self.session(headless)?;
        let dom = s.extract_simplified_dom()?;
        let json_str = dom.to_json()?;

        let msg = format!(
            "DOM — {} elements, {} interactive.\n{}",
            dom.count_elements(),
            dom.count_interactive(),
            json_str
        );

        let output = if msg.len() > max_chars {
            format!(
                "{}\n\n[DOM truncated — {} of {} chars shown]",
                &msg[..max_chars],
                max_chars,
                msg.len()
            )
        } else {
            msg
        };

        Ok(ToolResult::ok(output))
    }

    /// screenshot — capture the current viewport.
    ///
    /// `BrowserSession` has no `.screenshot()` method — we go through the
    /// underlying `headless_chrome` Tab handle which exposes `capture_screenshot`.
    /// Saves PNG to `<workspace>/screenshots/<unix_ts>.png` AND returns base64.
    fn do_screenshot(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let s = self.session(headless)?;

        // Give the page a moment to finish rendering
        let settle_ms = args
            .get("settle_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(500);
        std::thread::sleep(Duration::from_millis(settle_ms));

        let png = s
            .tab()
            .capture_screenshot(
                headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
                None, // quality (jpeg only)
                None, // clip rect
                true, // from_surface
            )
            .map_err(|e| anyhow::anyhow!("Screenshot failed: {}", e))?;

        // Persist to disk for debugging / auditing
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = format!("{}/screenshots/{}.png", self.workspace_dir, ts);
        if let Err(e) = std::fs::write(&path, &png) {
            tracing::warn!("Could not save screenshot to {}: {}", path, e);
        } else {
            tracing::info!("Screenshot saved → {}", path);
        }

        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        Ok(ToolResult::ok(format!(
            "screenshot_path:{}\ndata:image/png;base64,{}",
            path, b64
        )))
    }

    /// eval — execute arbitrary JavaScript in the page context.
    fn do_eval(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let script = match args.get("script").and_then(|v| v.as_str()) {
            Some(s) => s.to_owned(),
            None => return Ok(ToolResult::fail("Missing 'script' for eval")),
        };

        let s = self.session(headless)?;
        let result = s
            .tab()
            .evaluate(&script, false)
            .map_err(|e| anyhow::anyhow!("eval failed: {}", e))?;
        Ok(ToolResult::ok(format!("{:?}", result.value)))
    }

    /// execute — pass-through to any browser-use tool by name.
    fn do_tool_execute(&self, args: &Value, headless: Option<bool>) -> Result<ToolResult> {
        let tool_name = match args.get("tool_name").and_then(|v| v.as_str()) {
            Some(t) => t.to_owned(),
            None => return Ok(ToolResult::fail("Missing 'tool_name' for execute")),
        };
        let tool_args = args
            .get("tool_args")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        let s = self.session(headless)?;
        Self::map_browser_result(s.execute_tool(&tool_name, tool_args), &tool_name)
    }

    /// close — gracefully shut down the browser session.
    ///
    /// `BrowserSession` has no `.close()` method — its `Drop` impl shuts the
    /// browser down. We use `Arc::try_unwrap` so we only drop when we hold the
    /// last reference (avoids closing a session still in use by another clone).
    fn do_close(&self) -> Result<ToolResult> {
        let mut guard = self.session_store.lock();
        match guard.take() {
            None => Ok(ToolResult::ok("No active browser session to close")),
            Some(arc) => match Arc::try_unwrap(arc) {
                Ok(_session) => {
                    // Drop triggers BrowserSession's Drop impl → browser exits cleanly
                    tracing::info!("Browser session closed.");
                    Ok(ToolResult::ok("Browser session closed"))
                }
                Err(still_shared) => {
                    *guard = Some(still_shared);
                    tracing::warn!(
                        "Browser session has other active references; not force-closing."
                    );
                    Ok(ToolResult::fail(
                        "Session still has active references — cannot close yet",
                    ))
                }
            },
        }
    }
}

// ── Tool trait impl ──────────────────────────────────────────────

impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Full browser automation via CDP. Auto-detects Chrome / Edge / Brave and uses a \
         dedicated OpenPaw profile isolated from the user's real browser data. \
         DOM is returned as a compact indexed element map (e.g. @e3 [button] 'Submit'). \
         Actions: navigate, click, type, scroll, hover, select, check, uncheck, key_press, \
         wait, get_text, drag_and_drop, back, forward, new_tab, switch_tab, \
         read_dom, screenshot, eval, execute, close."
    }

    fn parameters_json(&self) -> String {
        r#"{
          "type": "object",
          "properties": {
            "action": {
              "type": "string",
              "enum": [
                "navigate", "read_page",
                "click", "type", "scroll", "hover",
                "select", "check", "uncheck", "key_press",
                "wait", "get_text", "drag_and_drop",
                "back", "forward",
                "new_tab", "switch_tab",
                "read_dom", "screenshot", "eval",
                "execute", "close"
              ],
              "description": "Action to perform"
            },

            "url":      { "type": "string",  "description": "URL for navigate / new_tab" },
            "selector": { "type": "string",  "description": "CSS selector or @eN element index" },
            "text":     { "type": "string",  "description": "Text to type" },
            "append":   { "type": "boolean", "description": "If true, append text instead of replacing (default false)" },
            "value":    { "type": "string",  "description": "Option value for select" },
            "key":      { "type": "string",  "description": "Key or chord for key_press, e.g. 'Enter', 'Control+a'" },

            "direction":  { "type": "string",  "enum": ["up","down","left","right"], "description": "Scroll direction (default 'down')" },
            "amount":     { "type": "integer", "description": "Scroll amount in pixels (default 300)" },

            "source":     { "type": "string",  "description": "Source selector for drag_and_drop" },
            "target":     { "type": "string",  "description": "Target selector for drag_and_drop" },

            "ms":         { "type": "integer", "description": "Milliseconds to wait (default 1000, max 10000)" },
            "timeout_ms": { "type": "integer", "description": "Timeout ms when waiting for an element (default 5000)" },

            "tab_index":    { "type": "integer", "description": "0-based tab index for switch_tab" },
            "url_contains": { "type": "string",  "description": "URL fragment to identify tab for switch_tab" },
            "switch":       { "type": "boolean", "description": "Switch to new tab after opening (default true)" },

            "max_chars":      { "type": "integer", "description": "Max chars for read_page (default 20000)" },
            "max_dom_length": { "type": "integer", "description": "Max chars of DOM output (default 10000, max 100000)" },
            "settle_ms":      { "type": "integer", "description": "ms to wait before screenshot (default 500)" },

            "headless":  { "type": "boolean", "description": "Override default headless mode" },
            "script":    { "type": "string", "description": "JavaScript to evaluate for eval action" },
            "tool_name": { "type": "string", "description": "browser-use tool name for execute action" },
            "tool_args": { "type": "object", "description": "Arguments for execute action" }
          },
          "required": ["action"]
        }"#
        .to_string()
    }

    fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let headless = args.get("headless").and_then(|v| v.as_bool());

        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return Ok(ToolResult::fail("Missing 'action'")),
        };

        match action {
            "navigate" => self.do_navigate(&args, headless),
            "read_page" => self.do_read_page(&args, headless),
            "click" => self.do_click(&args, headless),
            "type" => self.do_type(&args, headless),
            "scroll" => self.do_scroll(&args, headless),
            "hover" => self.do_hover(&args, headless),
            "select" => self.do_select(&args, headless),
            "check" => self.do_check(&args, true, headless),
            "uncheck" => self.do_check(&args, false, headless),
            "key_press" => self.do_key_press(&args, headless),
            "wait" => self.do_wait(&args, headless),
            "get_text" => self.do_get_text(&args, headless),
            "drag_and_drop" => self.do_drag_and_drop(&args, headless),
            "back" => self.do_history("back", headless),
            "forward" => self.do_history("forward", headless),
            "new_tab" => self.do_new_tab(&args, headless),
            "switch_tab" => self.do_switch_tab(&args, headless),
            "read_dom" => self.do_read_dom(&args, headless),
            "screenshot" => self.do_screenshot(&args, headless),
            "eval" => self.do_eval(&args, headless),
            "execute" => self.do_tool_execute(&args, headless),
            "close" => self.do_close(),
            _ => Ok(ToolResult::fail(format!("Unknown action '{}'", action))),
        }
    }
}
