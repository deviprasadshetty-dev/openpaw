use super::{Tool, ToolContext, ToolResult};
use super::cdp::CdpClient;
use crate::config_types::BrowserConfig;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct BrowserTool {
    workspace_dir: String,
    cdp_host: String,
    cdp_port: u16,
    headless: bool,
    browser_path: Option<String>,
    profile_dir: Option<String>,
    auto_launch: bool,
    client: Arc<Mutex<Option<CdpClient>>>,
    browser_process: Arc<Mutex<Option<std::process::Child>>>,
}

impl BrowserTool {
    pub fn new(workspace_dir: impl Into<String>, config: &BrowserConfig) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
            cdp_host: config.cdp_host.clone(),
            cdp_port: config.cdp_port,
            headless: config.native_headless,
            browser_path: config.native_chrome_path.clone(),
            profile_dir: config.profile_dir.clone(),
            auto_launch: config.cdp_auto_launch,
            client: Arc::new(Mutex::new(None)),
            browser_process: Arc::new(Mutex::new(None)),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}:{}", self.cdp_host, self.cdp_port)
    }

    async fn get_client(&self) -> Result<CdpClient> {
        {
            let guard = self.client.lock().await;
            if guard.is_some() {
                return Ok(guard.as_ref().unwrap().clone());
            }
        }

        let endpoint = self.endpoint();
        let cdp = CdpClient::new(&endpoint);

        if let Err(e) = cdp.connect().await {
            if self.auto_launch {
                tracing::info!("Browser not reachable on {}, auto-launching", endpoint);
                let mut proc_guard = self.browser_process.lock().await;
                let child = super::cdp::launch_browser(
                    self.cdp_port,
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
                            tracing::debug!("Browser not ready yet, retrying... ({}/5)", attempt + 1);
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

        let mut guard = self.client.lock().await;
        *guard = Some(cdp.clone());
        Ok(cdp)
    }

    fn screenshot_path(&self, name: &str) -> String {
        let path = Path::new(&self.workspace_dir).join("screenshots").join(name);
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        path.to_string_lossy().to_string()
    }

    fn default_screenshot_path(&self, suffix: &str) -> String {
        let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let filename = format!("screenshot_{}{}.png", ts, suffix);
        self.screenshot_path(&filename)
    }

    fn save_base64_image(&self, data: &str, path: &str) -> Result<String> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let bytes = STANDARD.decode(data)?;
        std::fs::write(path, bytes)?;
        Ok(path.to_string())
    }
}

impl Clone for BrowserTool {
    fn clone(&self) -> Self {
        Self {
            workspace_dir: self.workspace_dir.clone(),
            cdp_host: self.cdp_host.clone(),
            cdp_port: self.cdp_port,
            headless: self.headless,
            browser_path: self.browser_path.clone(),
            profile_dir: self.profile_dir.clone(),
            auto_launch: self.auto_launch,
            client: self.client.clone(),
            browser_process: self.browser_process.clone(),
        }
    }
}

unsafe impl Send for BrowserTool {}
unsafe impl Sync for BrowserTool {}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Browser automation via Chrome DevTools Protocol (CDP). Directly controls a Chrome instance \
         over WebSocket — no external CLI required. \
         Core flow: navigate → snapshot/get DOM → interact with selectors → re-snapshot. \
         Actions: navigate, click, dblclick, type, fill, scroll, hover, focus, check, uncheck, \
         scrollintoview, screenshot, screenshot_element, pdf, read_page, read_dom, \
         back, forward, refresh, get_url, get_title, get_text, get_html, get_value, get_attr, \
         get_count, get_box, get_styles, is_visible, is_enabled, is_checked, eval, \
         select_option, key_press, mouse_move, mouse_click, mouse_wheel, \
         get_cookies, set_cookie, clear_cookies, storage_local, storage_session, \
         wait, wait_text, wait_load, close, set_viewport, set_device, set_geo, \
         set_offline, set_headers, set_media, tab_new, tab_list, tab_close, tab_switch, \
         drag, snapshot."
    }

    fn parameters_json(&self) -> String {
        r#"{
          "type": "object",
          "properties": {
            "action": {
              "type": "string",
              "enum": ["navigate", "click", "dblclick", "type", "fill", "scroll", "hover", "focus", "check", "uncheck", "scrollintoview", "screenshot", "screenshot_element", "pdf", "read_page", "read_dom", "back", "forward", "refresh", "get_url", "get_title", "get_text", "get_html", "get_value", "get_attr", "get_count", "get_box", "get_styles", "is_visible", "is_enabled", "is_checked", "eval", "select_option", "key_press", "mouse_move", "mouse_click", "mouse_wheel", "get_cookies", "set_cookie", "clear_cookies", "storage_local", "storage_session", "wait", "wait_text", "wait_load", "close", "set_viewport", "set_device", "set_geo", "set_offline", "set_headers", "set_media", "tab_new", "tab_list", "tab_close", "tab_switch", "drag", "snapshot"],
              "description": "Action to perform"
            },
            "url":       { "type": "string",  "description": "URL for navigate/tab_new" },
            "selector":  { "type": "string",  "description": "CSS selector" },
            "text":      { "type": "string",  "description": "Text to type/eval/wait for, or JS code for eval" },
            "tab_id":    { "type": "integer", "description": "Tab target ID or index" },
            "direction": { "type": "string",  "enum": ["up", "down", "left", "right"], "description": "Scroll direction" },
            "amount":     { "type": "integer", "description": "Scroll amount or snapshot depth" },
            "key":       { "type": "string",  "description": "Key name for key_press" },
            "option":    { "type": "string",  "description": "Value for select_option" },
            "attribute": { "type": "string",  "description": "Attribute name for get_attr" },
            "device":    { "type": "string",  "description": "Device name for set_device" },
            "latitude":  { "type": "number",  "description": "Latitude for set_geo" },
            "longitude": { "type": "number",  "description": "Longitude for set_geo" },
            "cookie_name":   { "type": "string",  "description": "Cookie or storage key name" },
            "cookie_value":  { "type": "string",  "description": "Cookie or storage value" },
            "x":           { "type": "integer",  "description": "X coordinate for mouse" },
            "y":           { "type": "integer",  "description": "Y coordinate for mouse" },
            "button":      { "type": "string", "enum": ["left", "right", "middle"], "description": "Mouse button" },
            "to_selector": { "type": "string",  "description": "Target selector for drag" },
            "compact":     { "type": "boolean", "description": "Compact snapshot output" },
            "depth":        { "type": "integer", "description": "Max depth for snapshot" },
            "headers":     { "type": "string",  "description": "JSON string of headers for set_headers" },
            "media_scheme":{ "type": "string",  "enum": ["dark", "light", "no-preference"], "description": "Color scheme" },
            "width":       { "type": "integer",  "description": "Viewport width" },
            "height":      { "type": "integer",  "description": "Viewport height" },
            "use_base64":  { "type": "boolean", "description": "Base64-encode JS for eval (avoid shell escaping)" },
            "path":        { "type": "string",  "description": "File path for screenshot/pdf download" },
            "ms":          { "type": "integer", "description": "Wait duration in ms" },
            "load_state":  { "type": "string",  "enum": ["load", "domcontentloaded", "networkidle"], "description": "Load state to wait for" }
          },
          "required": ["action"]
        }"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");

        let cdp = match self.get_client().await {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::fail(format!("CDP connection error: {}", e))),
        };

        macro_rules! cdp_err {
            ($result:expr) => {
                match $result {
                    Ok(v) => v,
                    Err(e) => return Ok(ToolResult::fail(format!("{}", e))),
                }
            };
        }

        match action {
            "navigate" => {
                let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("about:blank");
                cdp_err!(cdp.navigate(url).await);
                Ok(ToolResult::ok(format!("Navigated to {}", url)))
            }
            "click" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.click(selector).await);
                Ok(ToolResult::ok(format!("Clicked {}", selector)))
            }
            "dblclick" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.dblclick(selector).await);
                Ok(ToolResult::ok(format!("Double-clicked {}", selector)))
            }
            "type" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.type_text(selector, text).await);
                Ok(ToolResult::ok(format!("Typed into {}", selector)))
            }
            "fill" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.fill(selector, text).await);
                Ok(ToolResult::ok(format!("Filled {}", selector)))
            }
            "scroll" => {
                let direction = args.get("direction").and_then(|v| v.as_str()).unwrap_or("down");
                let amount = args.get("amount").and_then(|v| v.as_i64()).unwrap_or(500);
                cdp_err!(cdp.scroll(direction, amount).await);
                Ok(ToolResult::ok(format!("Scrolled {} {}", direction, amount)))
            }
            "hover" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.hover(selector).await);
                Ok(ToolResult::ok(format!("Hovered over {}", selector)))
            }
            "focus" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.focus(selector).await);
                Ok(ToolResult::ok(format!("Focused {}", selector)))
            }
            "check" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.check(selector).await);
                Ok(ToolResult::ok(format!("Checked {}", selector)))
            }
            "uncheck" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.uncheck(selector).await);
                Ok(ToolResult::ok(format!("Unchecked {}", selector)))
            }
            "scrollintoview" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.scroll_into_view(selector).await);
                Ok(ToolResult::ok(format!("Scrolled {} into view", selector)))
            }
            "screenshot" => {
                let result = cdp_err!(cdp.screenshot("png", None).await);
                let data = result
                    .get("data")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                // ── 0.6: Validate screenshot save path ───────────────────────
                let path = if let Some(user_path) = args.get("path").and_then(|v| v.as_str()) {
                    // If user specified a path, ensure it stays within workspace
                    let candidate = if Path::new(user_path).is_absolute() {
                        std::path::PathBuf::from(user_path)
                    } else {
                        Path::new(&self.workspace_dir).join(user_path)
                    };
                    // Resolve as much of the path as possible for traversal check
                    let parent = candidate.parent().unwrap_or(&candidate);
                    let ws_root = std::fs::canonicalize(&self.workspace_dir)
                        .unwrap_or_else(|_| std::path::PathBuf::from(&self.workspace_dir));
                    // If parent already exists we can canonicalize it; otherwise check string prefix
                    let resolved_parent = std::fs::canonicalize(parent)
                        .unwrap_or_else(|_| parent.to_path_buf());
                    if !resolved_parent.starts_with(&ws_root) {
                        return Ok(ToolResult::fail(format!(
                            "Screenshot path '{}' is outside the workspace directory",
                            user_path
                        )));
                    }
                    candidate.to_string_lossy().into_owned()
                } else {
                    self.default_screenshot_path("")
                };

                match self.save_base64_image(data, &path) {
                    Ok(p) => Ok(ToolResult::ok(format!("Screenshot saved to: {}", p))),
                    Err(e) => Ok(ToolResult::fail(format!("Failed to save screenshot: {}", e))),
                }
            }
            "screenshot_element" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let result = cdp_err!(cdp.screenshot_element(selector, "png").await);
                let data = result
                    .get("data")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
                let path = self.screenshot_path(&format!("element_{}.png", ts));
                match self.save_base64_image(data, &path) {
                    Ok(p) => Ok(ToolResult::ok(format!("Element screenshot saved to: {}", p))),
                    Err(e) => Ok(ToolResult::fail(format!("Failed to save screenshot: {}", e))),
                }
            }
            "pdf" => {
                let result = cdp_err!(cdp.print_pdf().await);
                let data = result
                    .get("data")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
                let downloads = Path::new(&self.workspace_dir).join("downloads");
                let _ = std::fs::create_dir_all(&downloads);
                let path = downloads.join(format!("page_{}.pdf", ts));
                use base64::{Engine, engine::general_purpose::STANDARD};
                match STANDARD.decode(data) {
                    Ok(bytes) => {
                        std::fs::write(&path, bytes)?;
                        Ok(ToolResult::ok(format!("PDF saved to: {}", path.display())))
                    }
                    Err(e) => Ok(ToolResult::fail(format!("Failed to decode PDF: {}", e))),
                }
            }
            "read_page" => {
                let text = cdp_err!(cdp.get_text("body").await);
                Ok(ToolResult::ok(text))
            }
            "read_dom" => {
                let dom = cdp_err!(cdp.snapshot_dom(Some(-1)).await);
                Ok(ToolResult::ok(serde_json::to_string_pretty(&dom).unwrap_or_default()))
            }
            "back" => {
                cdp_err!(cdp.go_back().await);
                Ok(ToolResult::ok("Navigated back".to_string()))
            }
            "forward" => {
                cdp_err!(cdp.go_forward().await);
                Ok(ToolResult::ok("Navigated forward".to_string()))
            }
            "refresh" => {
                cdp_err!(cdp.reload().await);
                Ok(ToolResult::ok("Page refreshed".to_string()))
            }
            "get_url" => {
                let url = cdp_err!(cdp.get_current_url().await);
                Ok(ToolResult::ok(url))
            }
            "get_title" => {
                let title = cdp_err!(cdp.get_title().await);
                Ok(ToolResult::ok(title))
            }
            "get_text" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
                let text = cdp_err!(cdp.get_text(s).await);
                Ok(ToolResult::ok(text))
            }
            "get_html" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
                let html = cdp_err!(cdp.get_html(s).await);
                Ok(ToolResult::ok(html))
            }
            "get_value" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let val = cdp_err!(cdp.get_value(s).await);
                Ok(ToolResult::ok(val))
            }
            "get_attr" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let a = args.get("attribute").and_then(|v| v.as_str()).unwrap_or("");
                let val = cdp_err!(cdp.get_attribute(s, a).await);
                Ok(ToolResult::ok(val))
            }
            "get_count" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let count = cdp_err!(cdp.get_count(s).await);
                Ok(ToolResult::ok(count.to_string()))
            }
            "get_box" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let box_val = cdp_err!(cdp.get_bounding_box(s).await);
                Ok(ToolResult::ok(serde_json::to_string_pretty(&box_val).unwrap_or_default()))
            }
            "get_styles" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let styles = cdp_err!(cdp.get_computed_styles(s).await);
                Ok(ToolResult::ok(serde_json::to_string_pretty(&styles).unwrap_or_default()))
            }
            "is_visible" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let visible = cdp_err!(cdp.is_visible(s).await);
                Ok(ToolResult::ok(visible.to_string()))
            }
            "is_enabled" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let enabled = cdp_err!(cdp.is_enabled(s).await);
                Ok(ToolResult::ok(enabled.to_string()))
            }
            "is_checked" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let checked = cdp_err!(cdp.is_checked(s).await);
                Ok(ToolResult::ok(checked.to_string()))
            }
            "eval" => {
                let js_code = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let use_base64 = args
                    .get("use_base64")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let js = if use_base64 {
                    use base64::{Engine, engine::general_purpose::STANDARD};
                    match STANDARD.decode(js_code) {
                        Ok(decoded) => String::from_utf8_lossy(&decoded).to_string(),
                        Err(e) => return Ok(ToolResult::fail(format!("Base64 decode error: {}", e))),
                    }
                } else {
                    js_code.to_string()
                };
                let result = cdp_err!(cdp.evaluate(&js, true).await);
                if let Some(err) = result.get("exceptionDetails") {
                    Ok(ToolResult::fail(format!("JS error: {}", serde_json::to_string_pretty(err).unwrap_or_default())))
                } else {
                    let val = result
                        .get("result")
                        .and_then(|r| r.get("value"))
                        .cloned()
                        .unwrap_or(json!(null));
                    Ok(ToolResult::ok(serde_json::to_string_pretty(&val).unwrap_or_default()))
                }
            }
            "select_option" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let o = args.get("option").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.select_option(s, o).await);
                Ok(ToolResult::ok(format!("Selected {} in {}", o, s)))
            }
            "key_press" => {
                let k = args.get("key").and_then(|v| v.as_str()).unwrap_or("Enter");
                cdp_err!(cdp.key_press(k, None).await);
                Ok(ToolResult::ok(format!("Pressed key {}", k)))
            }
            "mouse_move" => {
                let x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                cdp_err!(cdp.mouse_move(x, y).await);
                Ok(ToolResult::ok(format!("Mouse moved to ({},{})", x, y)))
            }
            "mouse_click" => {
                let button = args.get("button").and_then(|v| v.as_str()).unwrap_or("left");
                let x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                cdp_err!(cdp.mouse_click(x, y, button, 1).await);
                Ok(ToolResult::ok(format!("Clicked {} at ({},{})", button, x, y)))
            }
            "mouse_wheel" => {
                let a = args.get("amount").and_then(|v| v.as_f64()).unwrap_or(100.0);
                cdp_err!(cdp.mouse_wheel(0.0, 0.0, 0.0, a).await);
                Ok(ToolResult::ok(format!("Mouse wheel: {}", a)))
            }
            "get_cookies" => {
                let result = cdp_err!(cdp.get_cookies().await);
                Ok(ToolResult::ok(serde_json::to_string_pretty(&result).unwrap_or_default()))
            }
            "set_cookie" => {
                let n = args
                    .get("cookie_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let v = args
                    .get("cookie_value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                cdp_err!(cdp.set_cookie(n, v, None, None).await);
                Ok(ToolResult::ok(format!("Cookie {} set", n)))
            }
            "clear_cookies" => {
                cdp_err!(cdp.clear_cookies().await);
                Ok(ToolResult::ok("Cookies cleared".to_string()))
            }
            "storage_local" => {
                let key = args.get("cookie_name").and_then(|v| v.as_str());
                let val = args.get("cookie_value").and_then(|v| v.as_str());
                if let (Some(k), Some(v)) = (key, val) {
                    cdp_err!(cdp.set_local_storage(k, v).await);
                    Ok(ToolResult::ok(format!("Set localStorage[{}]", k)))
                } else if let Some(k) = key {
                    let result = cdp_err!(cdp.get_local_storage(Some(k)).await);
                    Ok(ToolResult::ok(serde_json::to_string_pretty(&result).unwrap_or_default()))
                } else {
                    let result = cdp_err!(cdp.get_local_storage(None).await);
                    Ok(ToolResult::ok(serde_json::to_string_pretty(&result).unwrap_or_default()))
                }
            }
            "storage_session" => {
                let key = args.get("cookie_name").and_then(|v| v.as_str());
                let val = args.get("cookie_value").and_then(|v| v.as_str());
                if let (Some(k), Some(v)) = (key, val) {
                    let expr = format!(
                        "sessionStorage.setItem({}, {})",
                        serde_json::to_string(k)?,
                        serde_json::to_string(v)?
                    );
                    cdp_err!(cdp.evaluate(&expr, false).await);
                    Ok(ToolResult::ok(format!("Set sessionStorage[{}]", k)))
                } else {
                    let expr = "JSON.stringify(Object.fromEntries(Array.from({...(sessionStorage.length && Object.getOwnPropertyNames(sessionStorage).map(k=>[k,sessionStorage.getItem(k)]))})))";
                    let result = cdp_err!(cdp.evaluate(expr, false).await);
                    Ok(ToolResult::ok(
                        result
                            .get("result")
                            .and_then(|r| r.get("value"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}")
                            .to_string(),
                    ))
                }
            }
            "wait" => {
                if let Some(ms) = args.get("ms").and_then(|v| v.as_u64()) {
                    tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
                    Ok(ToolResult::ok(format!("Waited {}ms", ms)))
                } else if let Some(s) = args.get("selector").and_then(|v| v.as_str()) {
                    cdp_err!(cdp.wait_for_selector(s, 10000).await);
                    Ok(ToolResult::ok(format!("Element {} appeared", s)))
                } else {
                    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                    Ok(ToolResult::ok("Waited 1000ms".to_string()))
                }
            }
            "wait_text" => {
                let t = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.wait_for_text(t, 10000).await);
                Ok(ToolResult::ok(format!("Text '{}' appeared", t)))
            }
            "wait_load" => {
                let ls = args
                    .get("load_state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("networkidle");
                if ls == "networkidle" {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                }
                cdp_err!(cdp.wait_for_load(ls).await);
                Ok(ToolResult::ok(format!("Page loaded: {}", ls)))
            }
            "close" => {
                cdp.disconnect().await?;
                let mut guard = self.client.lock().await;
                *guard = None;
                Ok(ToolResult::ok("Browser connection closed".to_string()))
            }
            "set_viewport" => {
                let w = args.get("width").and_then(|v| v.as_i64()).unwrap_or(1280);
                let h = args
                    .get("height")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(800);
                cdp_err!(cdp.set_viewport(w, h, None).await);
                Ok(ToolResult::ok(format!("Viewport set to {}x{}", w, h)))
            }
            "set_device" => {
                let d = args.get("device").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.emulate_device(d).await);
                Ok(ToolResult::ok(format!("Device emulation set to {}", d)))
            }
            "set_geo" => {
                let lat = args
                    .get("latitude")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let lon = args
                    .get("longitude")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                cdp_err!(cdp.set_geolocation(lat, lon).await);
                Ok(ToolResult::ok(format!("Geolocation set to {}, {}", lat, lon)))
            }
            "set_offline" => {
                cdp_err!(cdp.set_network_offline(true).await);
                Ok(ToolResult::ok("Network set to offline".to_string()))
            }
            "set_headers" => {
                let h = args.get("headers").and_then(|v| v.as_str()).unwrap_or("{}");
                let headers: Value = serde_json::from_str(h).unwrap_or(json!({}));
                cdp_err!(cdp.set_extra_http_headers(headers).await);
                Ok(ToolResult::ok("Extra headers set".to_string()))
            }
            "set_media" => {
                let m = args
                    .get("media_scheme")
                    .and_then(|v| v.as_str())
                    .unwrap_or("dark");
                cdp_err!(cdp.set_color_scheme(m).await);
                Ok(ToolResult::ok(format!("Media scheme set to {}", m)))
            }
            "tab_new" => {
                let u = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("about:blank");
                let target_id = cdp_err!(cdp.create_target(u).await);
                Ok(ToolResult::ok(format!("New tab created: {}", target_id)))
            }
            "tab_list" => {
                let targets = cdp_err!(cdp.list_targets().await);
                let lines: Vec<String> = targets
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("[{}] {} - {} ({})", i, t.id, t.title, t.url))
                    .collect();
                Ok(ToolResult::ok(lines.join("\n")))
            }
            "tab_close" => {
                let targets = cdp_err!(cdp.list_targets().await);
                if let Some(target_id) = args.get("tab_id").and_then(|v| v.as_u64()) {
                    let idx = target_id as usize;
                    if idx < targets.len() {
                        cdp_err!(cdp.close_target(&targets[idx].id).await);
                        Ok(ToolResult::ok(format!("Tab {} closed", idx)))
                    } else {
                        Ok(ToolResult::fail(format!("Tab index {} out of range", idx)))
                    }
                } else if let Some(last) = targets.last() {
                    cdp_err!(cdp.close_target(&last.id).await);
                    Ok(ToolResult::ok("Last tab closed".to_string()))
                } else {
                    Ok(ToolResult::fail("No tabs to close"))
                }
            }
            "tab_switch" => {
                let targets = cdp_err!(cdp.list_targets().await);
                if let Some(tab_id) = args.get("tab_id").and_then(|v| v.as_u64()) {
                    let idx = tab_id as usize;
                    if idx < targets.len() {
                        cdp_err!(cdp.activate_target(&targets[idx].id).await);
                        let mut guard = self.client.lock().await;
                        *guard = None;
                        drop(guard);
                        let _ = self.get_client().await?;
                        Ok(ToolResult::ok(format!("Switched to tab {}", idx)))
                    } else {
                        Ok(ToolResult::fail(format!("Tab index {} out of range", idx)))
                    }
                } else {
                    Ok(ToolResult::fail("Missing tab_id parameter"))
                }
            }
            "drag" => {
                let f = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let t = args
                    .get("to_selector")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                cdp_err!(cdp.drag(f, t).await);
                Ok(ToolResult::ok(format!("Dragged {} to {}", f, t)))
            }
            "snapshot" => {
                let compact = args
                    .get("compact")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let depth = args.get("depth").and_then(|v| v.as_i64());
                let depth_opt = depth.map(|d| d as i32);
                let text = cdp_err!(cdp.snapshot_text(compact, depth_opt).await);
                Ok(ToolResult::ok(text))
            }
            _ => Ok(ToolResult::fail(format!(
                "Action '{}' not supported via CDP. Available: navigate, click, dblclick, type, fill, scroll, hover, focus, check, uncheck, scrollintoview, screenshot, screenshot_element, pdf, read_page, read_dom, back, forward, refresh, get_url, get_title, get_text, get_html, get_value, get_attr, get_count, get_box, get_styles, is_visible, is_enabled, is_checked, eval, select_option, key_press, mouse_move, mouse_click, mouse_wheel, get_cookies, set_cookie, clear_cookies, storage_local, storage_session, wait, wait_text, wait_load, close, set_viewport, set_device, set_geo, set_offline, set_headers, set_media, tab_new, tab_list, tab_close, tab_switch, drag, snapshot",
                action
            ))),
        }
    }
}