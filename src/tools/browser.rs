use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use base64::Engine;
use browser_use::browser::BrowserSession;
use browser_use::browser::config::LaunchOptions;
use browser_use::tools::{ToolContext as BrowserToolContext, ToolRegistry};
use headless_chrome::Tab;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

// â”€â”€ Browser detection â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub fn find_browser() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("OPENPAW_BROWSER") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Some(p);
        }
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

fn strip_unc_prefix(path: &Path) -> String {
    match std::fs::canonicalize(path) {
        Ok(canon) => {
            let s = canon.to_string_lossy();
            if let Some(stripped) = s.strip_prefix(r"\\?\") {
                stripped.to_string()
            } else {
                s.into_owned()
            }
        }
        Err(_) => path.to_string_lossy().to_string(),
    }
}

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

// â”€â”€ Session & Tab Registry â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct TabRegistry {
    pub tabs: HashMap<u32, Arc<Tab>>,
    pub next_id: u32,
    pub active_id: u32,
}

impl TabRegistry {
    fn new(main_tab: Arc<Tab>) -> Self {
        let mut tabs = HashMap::new();
        tabs.insert(0, main_tab);
        Self {
            tabs,
            next_id: 1,
            active_id: 0,
        }
    }

    fn get_active(&self) -> Option<Arc<Tab>> {
        self.tabs.get(&self.active_id).cloned()
    }
}

struct ManagedSession {
    // Wrapped in Mutex so callers can obtain &mut BrowserSession for BrowserToolContext::new.
    session: Mutex<BrowserSession>,
    registry: Mutex<TabRegistry>,
    last_health_check_ms: AtomicU64,
}

impl ManagedSession {
    fn is_alive(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let last = self.last_health_check_ms.load(Ordering::SeqCst);
        if now - last < 2 {
            return true;
        }

        let alive = tokio::task::block_in_place(|| {
            let sess = self.session.lock().unwrap_or_else(PoisonError::into_inner);
            if let Ok(tab) = sess.tab() {
                tab.evaluate("1", false).is_ok()
            } else {
                false
            }
        });

        if alive {
            self.last_health_check_ms.store(now, Ordering::SeqCst);
        }
        alive
    }
}

struct SessionStore {
    inner: Mutex<Option<Arc<ManagedSession>>>,
}

impl SessionStore {
    const fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Arc<ManagedSession>>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

// â”€â”€ BrowserTool â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct BrowserTool {
    pub workspace_dir: String,
    session_store: SessionStore,
    registry: ToolRegistry,
}

impl BrowserTool {
    pub fn new(workspace_dir: impl Into<String>) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
            session_store: SessionStore::new(),
            registry: ToolRegistry::with_defaults(),
        }
    }

    fn session(&self, headless_override: Option<bool>) -> Result<Arc<ManagedSession>> {
        let mut guard = self.session_store.lock();

        if let Some(ref ms) = *guard {
            if ms.is_alive() {
                return Ok(Arc::clone(ms));
            }
            *guard = None;
        }

        let is_headless = headless_override.unwrap_or_else(|| {
            std::env::var("OPENPAW_BROWSER_HEADLESS")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(true)
        });

        let browser_path =
            find_browser().ok_or_else(|| anyhow::anyhow!("No Chromium-family browser found."))?;
        let profile_path = std::path::Path::new(&self.workspace_dir).join("browser-profile");
        let _ = std::fs::create_dir_all(&profile_path);
        let profile_dir = strip_unc_prefix(&profile_path);

        let screenshots_path = std::path::Path::new(&self.workspace_dir).join("screenshots");
        let _ = std::fs::create_dir_all(&screenshots_path);

        let downloads_path = std::path::Path::new(&self.workspace_dir).join("downloads");
        let _ = std::fs::create_dir_all(&downloads_path);

        let name = browser_display_name(&browser_path);
        tracing::info!("Launching {} with profile: {}", name, profile_dir);

        let config = LaunchOptions::new()
            .chrome_path(browser_path)
            .user_data_dir(profile_dir.into())
            .headless(is_headless);

        let session = tokio::task::block_in_place(|| BrowserSession::launch(config))?;
        let main_tab = session.tab()?.clone();

        let managed = Arc::new(ManagedSession {
            session: Mutex::new(session),
            registry: Mutex::new(TabRegistry::new(main_tab)),
            last_health_check_ms: AtomicU64::new(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            ),
        });

        *guard = Some(Arc::clone(&managed));
        Ok(managed)
    }

    fn solve_selector(&self, args: &Value) -> Option<Value> {
        if let Some(sel) = args.get("selector") {
            if let Some(s) = sel.as_str()
                && let Some(rest) = s.strip_prefix("@e")
                    && let Ok(idx) = rest.parse::<usize>() {
                        return Some(json!({ "index": idx }));
                    }
            return Some(sel.clone());
        }
        None
    }

    fn map_result(
        &self,
        res: Result<browser_use::tools::ToolResult, impl std::fmt::Display>,
        action: &str,
    ) -> Result<ToolResult> {
        match res {
            Ok(tr) => {
                if tr.success {
                    Ok(ToolResult::ok(format!("{:?}", tr.data)))
                } else {
                    Ok(ToolResult::fail(format!(
                        "{} failed: {:?}",
                        action, tr.error
                    )))
                }
            }
            Err(e) => Ok(ToolResult::fail(format!("{} error: {}", action, e))),
        }
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Advanced human-like browser automation. Supports multiple tabs, structured DOM extraction, and precision interactions. \
         Actions: navigate, click, type, scroll, hover, screenshot, read_page, read_dom, new_tab, switch_tab, close_tab, get_url, eval, select_option, key_press, get_cookies, clear_cookies, close."
    }

    fn parameters_json(&self) -> String {
        r#"{
          "type": "object",
          "properties": {
            "action": {
              "type": "string",
              "enum": ["navigate", "click", "type", "scroll", "hover", "screenshot", "read_page", "read_dom", "new_tab", "switch_tab", "close_tab", "get_url", "eval", "select_option", "key_press", "get_cookies", "clear_cookies", "wait", "close", "set_viewport", "alert_accept", "alert_dismiss", "drag"],
              "description": "Action to perform"
            },
            "url":       { "type": "string",  "description": "URL for navigate/new_tab" },
            "selector":  { "type": "string",  "description": "CSS selector or @eN element index" },
            "text":      { "type": "string",  "description": "Text to type or JS to eval" },
            "tab_id":    { "type": "integer", "description": "ID of the tab to operate on" },
            "direction": { "type": "string",  "enum": ["up", "down", "left", "right"], "description": "Scroll direction" },
            "amount":     { "type": "integer", "description": "Scroll amount" },
            "ms":        { "type": "integer", "description": "Wait duration in ms" },
            "key":       { "type": "string",  "description": "Key name for key_press (e.g. Enter, Escape)" },
            "option":    { "type": "string",  "description": "Value or label for select_option" },
            "format":    { "type": "string",  "enum": ["file", "base64"], "default": "file", "description": "Output format for screenshot (default is file)" },
            "width":     { "type": "integer", "description": "Viewport width" },
            "height":    { "type": "integer", "description": "Viewport height" },
            "to_selector": { "type": "string", "description": "Target selector for drag" }
          },
          "required": ["action"]
        }"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let ms = self.session(None)?;

        match action {
            "new_tab" => {
                let url = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("about:blank");
                let new_tab = {
                    let mut session_locked = ms
                        .session
                        .lock()
                        .map_err(|e| anyhow::anyhow!("Mutex error: {}", e))?;
                    session_locked.new_tab()?
                };
                let mut reg = ms
                    .registry
                    .lock()
                    .map_err(|e| anyhow::anyhow!("Mutex error: {}", e))?;
                let id = reg.next_id;
                reg.tabs.insert(id, new_tab.clone());
                reg.next_id += 1;
                reg.active_id = id;
                drop(reg);
                new_tab.navigate_to(url)?;
                return Ok(ToolResult::ok(format!("New tab opened with ID: {}", id)));
            }
            "switch_tab" => {
                let tid = args
                    .get("tab_id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow::anyhow!("tab_id required"))?
                    as u32;
                let mut reg = ms
                    .registry
                    .lock()
                    .map_err(|e| anyhow::anyhow!("Mutex error: {}", e))?;
                return if reg.tabs.contains_key(&tid) {
                    reg.active_id = tid;
                    Ok(ToolResult::ok(format!("Switched to tab {}", tid)))
                } else {
                    Ok(ToolResult::fail(format!("Tab {} not found", tid)))
                };
            }
            "close_tab" => {
                let mut reg = ms
                    .registry
                    .lock()
                    .map_err(|e| anyhow::anyhow!("Mutex error: {}", e))?;
                let tid = args
                    .get("tab_id")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
                    .unwrap_or(reg.active_id);
                if tid == 0 && reg.tabs.len() == 1 {
                    return Ok(ToolResult::fail(
                        "Cannot close the only remaining tab. Use 'close' to terminate session.",
                    ));
                }
                return if let Some(tab) = reg.tabs.remove(&tid) {
                    let _ = tab.close(false);
                    if reg.active_id == tid {
                        reg.active_id = *reg.tabs.keys().next().unwrap();
                    }
                    Ok(ToolResult::ok(format!(
                        "Tab {} closed. Active tab is now {}",
                        tid, reg.active_id
                    )))
                } else {
                    Ok(ToolResult::fail(format!("Tab {} not found", tid)))
                };
            }
            _ => {}
        }

        if action == "wait" {
            let wait_ms = args.get("ms").and_then(|v| v.as_u64()).unwrap_or(1000);
            tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
            return Ok(ToolResult::ok(format!("Waited for {}ms", wait_ms)));
        }

        if action == "close" {
            let mut guard = self.session_store.lock();
            *guard = None;
            return Ok(ToolResult::ok("Session closed"));
        }

        let target_tab: Arc<Tab> = {
            let reg = ms
                .registry
                .lock()
                .map_err(|e| anyhow::anyhow!("Mutex error: {}", e))?;
            if let Some(tid) = args.get("tab_id").and_then(|v| v.as_u64()) {
                reg.tabs.get(&(tid as u32)).cloned()
            } else {
                reg.get_active()
            }
        }
        .ok_or_else(|| anyhow::anyhow!("Target tab not found"))?;

        let session_guard = ms
            .session
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex error: {}", e))?;
        let mut browser_ctx = BrowserToolContext::new(&session_guard);

        match action {
            "navigate" => {
                let url = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("about:blank");
                self.map_result(
                    self.registry
                        .execute("navigate", json!({"url": url}), &mut browser_ctx),
                    "navigate",
                )
            }
            "click" => {
                let sel = self.solve_selector(&args).unwrap_or(Value::Null);
                let tool_args = if sel.is_object() {
                    sel
                } else {
                    json!({"selector": sel})
                };
                self.map_result(
                    self.registry
                        .execute("click", tool_args, &mut browser_ctx),
                    "click",
                )
            }
            "type" => {
                let sel = self.solve_selector(&args).unwrap_or(Value::Null);
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let mut tool_args = if sel.is_object() {
                    sel
                } else {
                    json!({"selector": sel})
                };
                if let Some(obj) = tool_args.as_object_mut() {
                    obj.insert("value".into(), Value::String(text.into()));
                }
                self.map_result(
                    self.registry
                        .execute("fill", tool_args, &mut browser_ctx),
                    "type",
                )
            }
            "select_option" => {
                let sel = self.solve_selector(&args).unwrap_or(Value::Null);
                let val = args.get("option").and_then(|v| v.as_str()).unwrap_or("");
                let mut tool_args = if sel.is_object() {
                    sel
                } else {
                    json!({"selector": sel})
                };
                if let Some(obj) = tool_args.as_object_mut() {
                    obj.insert("value".into(), Value::String(val.into()));
                }
                let res = self
                    .registry
                    .execute("select_option", tool_args.clone(), &mut browser_ctx);
                match res {
                    Ok(tr) if tr.success => Ok(ToolResult::ok(format!("{:?}", tr.data))),
                    _ => self.map_result(
                        self.registry
                            .execute("fill", tool_args, &mut browser_ctx),
                        "select_option",
                    ),
                }
            }
            "key_press" => {
                let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("Enter");
                target_tab
                    .press_key(key)
                    .map_err(|e| anyhow::anyhow!("Key press failed: {}", e))?;
                Ok(ToolResult::ok(format!("Pressed key: {}", key)))
            }
            "scroll" => {
                let direction = args
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("down");
                let amount = args.get("amount").and_then(|v| v.as_u64()).unwrap_or(300);
                let tool_name = match direction {
                    "up" => "scroll_up",
                    "down" => "scroll_down",
                    "left" => "scroll_left",
                    "right" => "scroll_right",
                    _ => "scroll_down",
                };
                self.map_result(
                    self.registry
                        .execute(tool_name, json!({"amount": amount}), &mut browser_ctx),
                    "scroll",
                )
            }
            "hover" => {
                let sel = self.solve_selector(&args).unwrap_or(Value::Null);
                let tool_args = if sel.is_object() {
                    sel
                } else {
                    json!({"selector": sel})
                };
                self.map_result(
                    self.registry
                        .execute("hover", tool_args, &mut browser_ctx),
                    "hover",
                )
            }
            "screenshot" => {
                let format = args
                    .get("format")
                    .and_then(|v| v.as_str())
                    .unwrap_or("file");
                let png = target_tab
                    .capture_screenshot(
                        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
                        None,
                        None,
                        true,
                    )
                    .map_err(|e| anyhow::anyhow!("Screenshot failed: {}", e))?;

                if format == "base64" {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
                    Ok(ToolResult::ok(format!("data:image/png;base64,{}", b64)))
                } else {
                    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
                    let filename = format!("screenshot_{}.png", ts);
                    let path = Path::new(&self.workspace_dir)
                        .join("screenshots")
                        .join(&filename);
                    std::fs::write(&path, png)?;
                    Ok(ToolResult::ok(format!(
                        "Screenshot saved to: {}",
                        path.display()
                    )))
                }
            }
            "read_page" => {
                let html = target_tab
                    .get_content()
                    .map_err(|e| anyhow::anyhow!("Content failed: {}", e))?;
                let text = crate::tools::web_fetch::html_to_text(&html);
                Ok(ToolResult::ok(text))
            }
            "read_dom" => {
                let dom = session_guard
                    .extract_dom()
                    .map_err(|e| anyhow::anyhow!("read_dom failed: {}", e))?;
                Ok(ToolResult::ok(format!("{:?}", dom)))
            }
            "get_url" => Ok(ToolResult::ok(target_tab.get_url())),
            "eval" => {
                let script = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("window.location.href");
                let val = target_tab
                    .evaluate(script, false)
                    .map_err(|e| anyhow::anyhow!("Eval failed: {}", e))?;
                Ok(ToolResult::ok(format!("{:?}", val.value)))
            }
            "get_cookies" => {
                let cookies = target_tab
                    .get_cookies()
                    .map_err(|e| anyhow::anyhow!("Cookies failed: {}", e))?;
                Ok(ToolResult::ok(serde_json::to_string_pretty(&cookies)?))
            }
            "clear_cookies" => {
                target_tab.evaluate(
                    "document.cookie.split(';').forEach(c => \
                     document.cookie = c.replace(/^ +/, '').replace(/=.*/, \
                     '=;expires=' + new Date().toUTCString() + ';path=/'));",
                    false,
                )?;
                Ok(ToolResult::ok("Cookies for current domain cleared"))
            }
            "set_viewport" => {
                let w = args.get("width").and_then(|v| v.as_u64()).unwrap_or(1280) as u32;
                    let h = args.get("height").and_then(|v| v.as_u64()).unwrap_or(800) as u32;
                    target_tab
                        .set_bounds(headless_chrome::types::Bounds::Normal {
                            left: Some(0),
                            top: Some(0),
                            width: Some(w as f64),
                            height: Some(h as f64),
                        })
                        .map_err(|e| anyhow::anyhow!("set_viewport failed: {}", e))?;
                    Ok(ToolResult::ok(format!("Viewport set to {}x{}", w, h)))
                }
                "alert_accept" => {
                    target_tab
                        .get_dialog()
                        .accept(None)
                        .map_err(|e| anyhow::anyhow!("alert_accept failed: {}", e))?;
                    Ok(ToolResult::ok("Alert accepted"))
                }
                "alert_dismiss" => {
                    target_tab
                        .get_dialog()
                        .dismiss()
                        .map_err(|e| anyhow::anyhow!("alert_dismiss failed: {}", e))?;
                    Ok(ToolResult::ok("Alert dismissed"))
                }
                "drag" => {
                    let from_sel = match self.solve_selector(&args) {
                        Some(s) => s,
                        None => return Ok(ToolResult::fail("selector is required for drag")),
                    };
                    let to_sel_str = match args.get("to_selector").and_then(|v| v.as_str()) {
                        Some(s) => s.to_string(),
                        None => return Ok(ToolResult::fail("to_selector is required for drag")),
                    };

                    let from_css = if let Some(idx) = from_sel.get("index") {
                        format!(
                            "document.querySelectorAll('*')[{}]",
                            idx.as_u64().unwrap_or(0)
                        )
                    } else {
                        format!(
                            "document.querySelector('{}')",
                            from_sel.as_str().unwrap_or("")
                        )
                    };

                    let to_css = if let Some(rest) = to_sel_str.strip_prefix("@e") {
                        if let Ok(idx) = rest.parse::<usize>() {
                            format!("document.querySelectorAll('*')[{}]", idx)
                        } else {
                            format!("document.querySelector('{}')", to_sel_str)
                        }
                    } else {
                        format!("document.querySelector('{}')", to_sel_str)
                    };

                    let script = format!(
                        r#"(function() {{
                            var from = {from};
                            var to   = {to};
                            if (!from || !to) return null;
                            var fr = from.getBoundingClientRect();
                            var tr = to.getBoundingClientRect();
                            return JSON.stringify({{
                                fx: fr.left + fr.width  / 2,
                                fy: fr.top  + fr.height / 2,
                                tx: tr.left + tr.width  / 2,
                                ty: tr.top  + tr.height / 2
                            }});
                        }})()"#,
                        from = from_css,
                        to = to_css,
                    );

                    let result = target_tab
                        .evaluate(&script, false)
                        .map_err(|e| anyhow::anyhow!("drag coord resolution failed: {}", e))?;

                    let coords_str = match result.value {
                        Some(serde_json::Value::String(s)) => s,
                        _ => {
                            return Ok(ToolResult::fail(
                                "Could not resolve drag source or target element coordinates",
                            ));
                        }
                    };

                    let coords: serde_json::Value = serde_json::from_str(&coords_str)
                        .map_err(|e| anyhow::anyhow!("drag coord parse failed: {}", e))?;

                    let fx = coords["fx"].as_f64().unwrap_or(0.0);
                    let fy = coords["fy"].as_f64().unwrap_or(0.0);
                    let tx = coords["tx"].as_f64().unwrap_or(0.0);
                    let ty = coords["ty"].as_f64().unwrap_or(0.0);

                    let drag_script = format!(
                        r#"(function() {{
                            function mouseEvent(type, x, y, buttons) {{
                                document.elementFromPoint(x, y)?.dispatchEvent(
                                    new MouseEvent(type, {{bubbles:true, cancelable:true,
                                        clientX:x, clientY:y, buttons:buttons}})
                                );
                            }}
                            mouseEvent('mousemove',  {fx}, {fy}, 0);
                            mouseEvent('mousedown',  {fx}, {fy}, 1);
                            mouseEvent('dragstart',  {fx}, {fy}, 1);
                            mouseEvent('dragenter',  {tx}, {ty}, 1);
                            mouseEvent('dragover',   {tx}, {ty}, 1);
                            mouseEvent('drop',       {tx}, {ty}, 0);
                            mouseEvent('dragend',    {fx}, {fy}, 0);
                            mouseEvent('mouseup',    {tx}, {ty}, 0);
                        }})()"#,
                        fx = fx,
                        fy = fy,
                        tx = tx,
                        ty = ty
                    );

                    target_tab
                        .evaluate(&drag_script, false)
                        .map_err(|e| anyhow::anyhow!("drag dispatch failed: {}", e))?;

                    Ok(ToolResult::ok(format!(
                        "Dragged from ({:.0},{:.0}) to ({:.0},{:.0})",
                        fx, fy, tx, ty
                    )))
                }
                _ => Ok(ToolResult::fail(format!("Action {} not supported", action))),
            }
        }
    }
