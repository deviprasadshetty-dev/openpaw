#![cfg(feature = "browser")]
use super::cdp::CdpClient;
use super::path_security;
use super::{Tool, ToolContext, ToolResult};
use crate::config_types::BrowserConfig;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::{Value, json};
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
        let mut guard = self.client.lock().await;
        if let Some(ref c) = *guard {
            return Ok(c.clone());
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
                            tracing::debug!(
                                "Browser not ready yet, retrying... ({}/5)",
                                attempt + 1
                            );
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

    fn screenshot_path(&self, name: &str) -> String {
        let path = Path::new(&self.workspace_dir)
            .join("screenshots")
            .join(name);
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

    fn resolve_workspace_output_path(
        &self,
        raw: Option<&str>,
        default_path: String,
    ) -> Result<String> {
        let path = match raw {
            Some(p) if !p.trim().is_empty() => {
                if !path_security::is_path_safe(p) {
                    return Err(anyhow!("Unsafe output path: {}", p));
                }
                let candidate = Path::new(p);
                if candidate.is_absolute() {
                    candidate.to_path_buf()
                } else {
                    Path::new(&self.workspace_dir).join(candidate)
                }
            }
            _ => Path::new(&default_path).to_path_buf(),
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let workspace_root = std::fs::canonicalize(&self.workspace_dir)?;
        if !path_security::is_resolved_path_allowed(&path, &workspace_root, &[]) {
            return Err(anyhow!(
                "Output path must stay inside workspace: {}",
                path.display()
            ));
        }

        Ok(path.to_string_lossy().to_string())
    }

    fn validate_upload_files(&self, raw_files: &[String]) -> Result<Vec<String>> {
        let workspace_root = std::fs::canonicalize(&self.workspace_dir)?;
        let mut files = Vec::with_capacity(raw_files.len());

        for raw in raw_files {
            if !path_security::is_path_safe(raw) {
                return Err(anyhow!("Unsafe upload path: {}", raw));
            }
            let candidate = if Path::new(raw).is_absolute() {
                Path::new(raw).to_path_buf()
            } else {
                Path::new(&self.workspace_dir).join(raw)
            };
            if !path_security::is_resolved_path_allowed(&candidate, &workspace_root, &[]) {
                return Err(anyhow!("Upload file must stay inside workspace: {}", raw));
            }
            if !candidate.is_file() {
                return Err(anyhow!("Upload file not found: {}", candidate.display()));
            }
            files.push(candidate.to_string_lossy().to_string());
        }

        Ok(files)
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
         drag, snapshot, snapshot_structured, query_selector, find_xpath, \
         send_cdp, accept_dialog, dismiss_dialog, upload_file, \
         console_start, console_get, console_clear, \
         network_start, network_get, network_clear, \
         switch_to_frame, default_content, grant_permissions, clear_geo, \
         set_user_agent, set_zoom, set_network_conditions, set_download_behavior."
    }

    fn parameters_json(&self) -> String {
        r#"{
          "type": "object",
          "properties": {
            "action": {
              "type": "string",
              "enum": ["status", "navigate", "click", "dblclick", "type", "fill", "scroll", "hover", "focus", "check", "uncheck", "scrollintoview", "screenshot", "screenshot_element", "pdf", "read_page", "read_dom", "back", "forward", "refresh", "get_url", "get_title", "get_text", "get_html", "get_value", "get_attr", "get_count", "get_box", "get_styles", "is_visible", "is_enabled", "is_checked", "eval", "select_option", "key_press", "mouse_move", "mouse_click", "mouse_wheel", "get_cookies", "set_cookie", "clear_cookies", "storage_local", "storage_session", "wait", "wait_text", "wait_load", "close", "set_viewport", "set_device", "set_geo", "set_offline", "set_headers", "set_media", "tab_new", "tab_list", "tab_close", "tab_switch", "drag", "snapshot", "snapshot_structured", "query_selector", "find_xpath", "send_cdp", "accept_dialog", "dismiss_dialog", "upload_file", "console_start", "console_get", "console_clear", "network_start", "network_get", "network_clear", "switch_to_frame", "default_content", "grant_permissions", "clear_geo", "set_user_agent", "set_zoom", "set_network_conditions", "set_download_behavior"],
              "description": "Action to perform"
            },
            "url":            { "type": "string",  "description": "URL for navigate/tab_new" },
            "selector":       { "type": "string",  "description": "CSS selector (or XPath when selector_type=xpath)" },
            "text":           { "type": "string",  "description": "Text to type/eval/wait for, JS code for eval, CDP method for send_cdp, prompt response for accept_dialog" },
            "tab_id":         { "type": "string",  "description": "Tab target ID (string) or index (integer as string)" },
            "direction":      { "type": "string",  "enum": ["up", "down", "left", "right"], "description": "Scroll direction" },
            "amount":         { "type": "integer", "description": "Scroll amount or snapshot max depth" },
            "key":            { "type": "string",  "description": "Key name for key_press (e.g. Enter, Tab, Escape, ArrowDown, a, A)" },
            "modifiers":      { "type": "array", "items": {"type": "string"}, "description": "Modifier keys for key_press: Ctrl, Alt, Shift, Meta" },
            "delay":          { "type": "integer", "description": "Delay in ms between keystrokes for type action (0 = instant)" },
            "option":         { "type": "string",  "description": "Value for select_option" },
            "attribute":      { "type": "string",  "description": "Attribute name for get_attr" },
            "device":         { "type": "string",  "description": "Device name for set_device" },
            "latitude":       { "type": "number",  "description": "Latitude for set_geo" },
            "longitude":      { "type": "number",  "description": "Longitude for set_geo" },
            "cookie_name":    { "type": "string",  "description": "Cookie or storage key name" },
            "cookie_value":   { "type": "string",  "description": "Cookie or storage value" },
            "x":              { "type": "integer", "description": "X coordinate for mouse" },
            "y":              { "type": "integer", "description": "Y coordinate for mouse" },
            "button":         { "type": "string",  "enum": ["left", "right", "middle"], "description": "Mouse button" },
            "to_selector":    { "type": "string",  "description": "Target selector for drag" },
            "compact":        { "type": "boolean", "description": "Hide invisible elements in snapshot" },
            "depth":          { "type": "integer", "description": "Max depth for snapshot (default: 5 for structured, Infinity for text)" },
            "headers":        { "type": "string",  "description": "JSON string of headers for set_headers" },
            "media_scheme":   { "type": "string",  "enum": ["dark", "light", "no-preference"], "description": "Color scheme" },
            "width":          { "type": "integer", "description": "Viewport width" },
            "height":         { "type": "integer", "description": "Viewport height" },
            "use_base64":     { "type": "boolean", "description": "Base64-decode text param before use (for eval only)" },
            "return_base64":  { "type": "boolean", "description": "Return base64 image data inline for screenshot actions" },
            "path":           { "type": "string",  "description": "File path for screenshot/pdf/download" },
            "ms":             { "type": "integer", "description": "Wait duration in ms" },
            "load_state":     { "type": "string",  "enum": ["load", "domcontentloaded", "networkidle"], "description": "Load state to wait for" },
            "selector_type":  { "type": "string",  "enum": ["css", "xpath"], "description": "Selector type (default: css)" },
            "method":         { "type": "string",  "description": "CDP domain + method for send_cdp (e.g. 'Page.navigate')" },
            "params":         { "type": "string",  "description": "JSON string of CDP params for send_cdp" },
            "file_paths":     { "type": "array", "items": {"type": "string"}, "description": "Absolute file paths for upload_file" },
            "permissions":    { "type": "array", "items": {"type": "string"}, "description": "Permissions to grant for grant_permissions" },
            "latency":        { "type": "number",  "description": "Network latency in ms for set_network_conditions" },
            "download_throughput": { "type": "number", "description": "Download throughput bytes/s (-1 = unlimited)" },
            "upload_throughput":   { "type": "number", "description": "Upload throughput bytes/s (-1 = unlimited)" },
            "user_agent":     { "type": "string",  "description": "Custom user agent string for set_user_agent" },
            "zoom":           { "type": "number",  "description": "Page zoom factor (1.0 = normal)" },
            "behavior":       { "type": "string",  "enum": ["deny", "allow", "allowAndName", "default"], "description": "Download behavior" },
            "xpath":          { "type": "string",  "description": "XPath expression for find_xpath" }
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

        macro_rules! get_sel {
            ($args:expr) => {
                $args.get("selector").and_then(|v| v.as_str()).unwrap_or("")
            };
        }

        macro_rules! req_sel {
            ($args:expr) => {{
                let selector = $args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                if selector.trim().is_empty() {
                    return Ok(ToolResult::fail("Missing required 'selector' parameter"));
                }
                selector
            }};
        }

        match action {
            "status" => {
                let url = cdp_err!(cdp.get_current_url().await);
                let title = cdp_err!(cdp.get_title().await);
                let targets = cdp_err!(cdp.list_targets().await);
                Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&json!({
                        "connected": true,
                        "endpoint": self.endpoint(),
                        "url": url,
                        "title": title,
                        "tabs": targets.len()
                    }))
                    .unwrap_or_default(),
                ))
            }
            "navigate" => {
                let url = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("about:blank");
                cdp_err!(cdp.navigate(url).await);
                Ok(ToolResult::ok(format!("Navigated to {}", url)))
            }
            "click" => {
                let selector = req_sel!(args);
                cdp_err!(cdp.click(selector).await);
                Ok(ToolResult::ok(format!("Clicked {}", selector)))
            }
            "dblclick" => {
                let selector = req_sel!(args);
                cdp_err!(cdp.dblclick(selector).await);
                Ok(ToolResult::ok(format!("Double-clicked {}", selector)))
            }
            "type" => {
                let selector = req_sel!(args);
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let delay = args.get("delay").and_then(|v| v.as_u64()).unwrap_or(0);
                cdp_err!(cdp.clear_text_field(selector).await);
                for ch in text.chars() {
                    cdp_err!(cdp.keyboard_send_text(&ch.to_string()).await);
                    if delay > 0 {
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
                    }
                }
                cdp_err!(cdp.dispatch_input_change(selector).await);
                Ok(ToolResult::ok(format!("Typed into {}", selector)))
            }
            "fill" => {
                let selector = req_sel!(args);
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.fill(selector, text).await);
                Ok(ToolResult::ok(format!("Filled {}", selector)))
            }
            "scroll" => {
                let direction = args
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("down");
                let amount = args.get("amount").and_then(|v| v.as_i64()).unwrap_or(500);
                cdp_err!(cdp.scroll(direction, amount).await);
                Ok(ToolResult::ok(format!("Scrolled {} {}", direction, amount)))
            }
            "hover" => {
                let selector = req_sel!(args);
                cdp_err!(cdp.hover(selector).await);
                Ok(ToolResult::ok(format!("Hovered over {}", selector)))
            }
            "focus" => {
                let selector = req_sel!(args);
                cdp_err!(cdp.focus(selector).await);
                Ok(ToolResult::ok(format!("Focused {}", selector)))
            }
            "check" => {
                let selector = req_sel!(args);
                cdp_err!(cdp.check(selector).await);
                Ok(ToolResult::ok(format!("Checked {}", selector)))
            }
            "uncheck" => {
                let selector = req_sel!(args);
                cdp_err!(cdp.uncheck(selector).await);
                Ok(ToolResult::ok(format!("Unchecked {}", selector)))
            }
            "scrollintoview" => {
                let selector = req_sel!(args);
                cdp_err!(cdp.scroll_into_view(selector).await);
                Ok(ToolResult::ok(format!("Scrolled {} into view", selector)))
            }
            "screenshot" => {
                let result = cdp_err!(cdp.screenshot("png", None).await);
                let data = result.get("data").and_then(|v| v.as_str()).unwrap_or("");
                let return_base64 = args
                    .get("return_base64")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let path = match self
                    .resolve_workspace_output_path(path, self.default_screenshot_path(""))
                {
                    Ok(path) => path,
                    Err(e) => return Ok(ToolResult::fail(e.to_string())),
                };
                match self.save_base64_image(data, &path) {
                    Ok(p) => {
                        if return_base64 {
                            Ok(ToolResult::ok(format!(
                                "{{\"path\": \"{}\", \"base64\": \"{}\"}}",
                                p, data
                            )))
                        } else {
                            Ok(ToolResult::ok(format!("Screenshot saved to: {}", p)))
                        }
                    }
                    Err(e) => Ok(ToolResult::fail(format!(
                        "Failed to save screenshot: {}",
                        e
                    ))),
                }
            }
            "screenshot_element" => {
                let selector = req_sel!(args);
                let result = cdp_err!(cdp.screenshot_element(selector, "png").await);
                let data = result.get("data").and_then(|v| v.as_str()).unwrap_or("");
                let return_base64 = args
                    .get("return_base64")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let default_path = {
                    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
                    self.screenshot_path(&format!("element_{}.png", ts))
                };
                let path = match self.resolve_workspace_output_path(path, default_path) {
                    Ok(path) => path,
                    Err(e) => return Ok(ToolResult::fail(e.to_string())),
                };
                match self.save_base64_image(data, &path) {
                    Ok(p) => {
                        if return_base64 {
                            Ok(ToolResult::ok(format!(
                                "{{\"path\": \"{}\", \"base64\": \"{}\"}}",
                                p, data
                            )))
                        } else {
                            Ok(ToolResult::ok(format!(
                                "Element screenshot saved to: {}",
                                p
                            )))
                        }
                    }
                    Err(e) => Ok(ToolResult::fail(format!(
                        "Failed to save screenshot: {}",
                        e
                    ))),
                }
            }
            "pdf" => {
                let result = cdp_err!(cdp.print_pdf().await);
                let data = result.get("data").and_then(|v| v.as_str()).unwrap_or("");
                let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
                let default_path = Path::new(&self.workspace_dir)
                    .join("downloads")
                    .join(format!("page_{}.pdf", ts))
                    .to_string_lossy()
                    .to_string();
                let path = match self.resolve_workspace_output_path(
                    args.get("path").and_then(|v| v.as_str()),
                    default_path,
                ) {
                    Ok(path) => path,
                    Err(e) => return Ok(ToolResult::fail(e.to_string())),
                };
                use base64::{Engine, engine::general_purpose::STANDARD};
                match STANDARD.decode(data) {
                    Ok(bytes) => {
                        std::fs::write(&path, bytes)?;
                        Ok(ToolResult::ok(format!("PDF saved to: {}", path)))
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
                Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&dom).unwrap_or_default(),
                ))
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
                let s = args
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .unwrap_or("body");
                let text = cdp_err!(cdp.get_text(s).await);
                Ok(ToolResult::ok(text))
            }
            "get_html" => {
                let s = args
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .unwrap_or("body");
                let html = cdp_err!(cdp.get_html(s).await);
                Ok(ToolResult::ok(html))
            }
            "get_value" => {
                let s = req_sel!(args);
                let val = cdp_err!(cdp.get_value(s).await);
                Ok(ToolResult::ok(val))
            }
            "get_attr" => {
                let s = req_sel!(args);
                let a = args.get("attribute").and_then(|v| v.as_str()).unwrap_or("");
                if a.trim().is_empty() {
                    return Ok(ToolResult::fail("Missing required 'attribute' parameter"));
                }
                let val = cdp_err!(cdp.get_attribute(s, a).await);
                Ok(ToolResult::ok(val))
            }
            "get_count" => {
                let s = req_sel!(args);
                let count = cdp_err!(cdp.get_count(s).await);
                Ok(ToolResult::ok(count.to_string()))
            }
            "get_box" => {
                let s = req_sel!(args);
                let box_val = cdp_err!(cdp.get_bounding_box(s).await);
                Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&box_val).unwrap_or_default(),
                ))
            }
            "get_styles" => {
                let s = req_sel!(args);
                let styles = cdp_err!(cdp.get_computed_styles(s).await);
                Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&styles).unwrap_or_default(),
                ))
            }
            "is_visible" => {
                let s = req_sel!(args);
                let visible = cdp_err!(cdp.is_visible(s).await);
                Ok(ToolResult::ok(visible.to_string()))
            }
            "is_enabled" => {
                let s = req_sel!(args);
                let enabled = cdp_err!(cdp.is_enabled(s).await);
                Ok(ToolResult::ok(enabled.to_string()))
            }
            "is_checked" => {
                let s = req_sel!(args);
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
                        Err(e) => {
                            return Ok(ToolResult::fail(format!("Base64 decode error: {}", e)));
                        }
                    }
                } else {
                    js_code.to_string()
                };
                let result = cdp_err!(cdp.evaluate(&js, true).await);
                if let Some(err) = result.get("exceptionDetails") {
                    Ok(ToolResult::fail(format!(
                        "JS error: {}",
                        serde_json::to_string_pretty(err).unwrap_or_default()
                    )))
                } else {
                    let val = result
                        .get("result")
                        .and_then(|r| r.get("value"))
                        .cloned()
                        .unwrap_or(json!(null));
                    Ok(ToolResult::ok(
                        serde_json::to_string_pretty(&val).unwrap_or_default(),
                    ))
                }
            }
            "select_option" => {
                let s = req_sel!(args);
                let o = args.get("option").and_then(|v| v.as_str()).unwrap_or("");
                if o.trim().is_empty() {
                    return Ok(ToolResult::fail("Missing required 'option' parameter"));
                }
                cdp_err!(cdp.select_option(s, o).await);
                Ok(ToolResult::ok(format!("Selected {} in {}", o, s)))
            }
            "key_press" => {
                let k = args.get("key").and_then(|v| v.as_str()).unwrap_or("Enter");
                let modifiers = args.get("modifiers").and_then(|v| {
                    let arr = v.as_array()?;
                    let mut flags = 0i32;
                    for m in arr {
                        if let Some(s) = m.as_str() {
                            match s {
                                "Alt" => flags |= 1,
                                "Ctrl" => flags |= 2,
                                "Meta" => flags |= 4,
                                "Shift" => flags |= 8,
                                _ => {}
                            }
                        }
                    }
                    Some(flags)
                });
                cdp_err!(cdp.key_press(k, modifiers).await);
                Ok(ToolResult::ok(format!("Pressed key {}", k)))
            }
            "mouse_move" => {
                let x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                cdp_err!(cdp.mouse_move(x, y).await);
                Ok(ToolResult::ok(format!("Mouse moved to ({},{})", x, y)))
            }
            "mouse_click" => {
                let button = args
                    .get("button")
                    .and_then(|v| v.as_str())
                    .unwrap_or("left");
                let x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                cdp_err!(cdp.mouse_click(x, y, button, 1).await);
                Ok(ToolResult::ok(format!(
                    "Clicked {} at ({},{})",
                    button, x, y
                )))
            }
            "mouse_wheel" => {
                let a = args.get("amount").and_then(|v| v.as_f64()).unwrap_or(100.0);
                cdp_err!(cdp.mouse_wheel(0.0, 0.0, 0.0, a).await);
                Ok(ToolResult::ok(format!("Mouse wheel: {}", a)))
            }
            "get_cookies" => {
                let result = cdp_err!(cdp.get_cookies().await);
                Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&result).unwrap_or_default(),
                ))
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
                    Ok(ToolResult::ok(
                        serde_json::to_string_pretty(&result).unwrap_or_default(),
                    ))
                } else {
                    let result = cdp_err!(cdp.get_local_storage(None).await);
                    Ok(ToolResult::ok(
                        serde_json::to_string_pretty(&result).unwrap_or_default(),
                    ))
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
                } else if let Some(k) = key {
                    let expr = format!(
                        "JSON.stringify({})",
                        format!("sessionStorage.getItem({})", serde_json::to_string(k)?)
                    );
                    let result = cdp_err!(cdp.evaluate(&expr, false).await);
                    Ok(ToolResult::ok(
                        result
                            .get("result")
                            .and_then(|r| r.get("value"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("null")
                            .to_string(),
                    ))
                } else {
                    let expr = "JSON.stringify(Object.fromEntries(Array.from({length: sessionStorage.length}, (_, i) => { const k = sessionStorage.key(i); return [k, sessionStorage.getItem(k)]; })))";
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
                cdp_err!(cdp.disable_fetch_interception().await);
                cdp.disconnect().await?;
                let mut guard = self.client.lock().await;
                *guard = None;
                drop(guard);
                let mut proc_guard = self.browser_process.lock().await;
                if let Some(mut child) = proc_guard.take() {
                    tracing::info!("Killing browser process (pid {})", child.id());
                    let _ = child.kill();
                    let _ = child.wait();
                }
                Ok(ToolResult::ok(
                    "Browser connection closed, process killed".to_string(),
                ))
            }
            "set_viewport" => {
                let w = args.get("width").and_then(|v| v.as_i64()).unwrap_or(1280);
                let h = args.get("height").and_then(|v| v.as_i64()).unwrap_or(800);
                cdp_err!(cdp.set_viewport(w, h, None).await);
                Ok(ToolResult::ok(format!("Viewport set to {}x{}", w, h)))
            }
            "set_device" => {
                let d = args.get("device").and_then(|v| v.as_str()).unwrap_or("");
                cdp_err!(cdp.emulate_device(d).await);
                Ok(ToolResult::ok(format!("Device emulation set to {}", d)))
            }
            "set_geo" => {
                let lat = args.get("latitude").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let lon = args
                    .get("longitude")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                cdp_err!(cdp.set_geolocation(lat, lon).await);
                Ok(ToolResult::ok(format!(
                    "Geolocation set to {}, {}",
                    lat, lon
                )))
            }
            "clear_geo" => {
                cdp_err!(cdp.clear_geolocation_override().await);
                Ok(ToolResult::ok("Geolocation override cleared".to_string()))
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
                let tab_id = args.get("tab_id").and_then(|v| v.as_str());
                if let Some(tid) = tab_id {
                    if let Ok(idx) = tid.parse::<usize>() {
                        if idx < targets.len() {
                            cdp_err!(cdp.close_target(&targets[idx].id).await);
                            Ok(ToolResult::ok(format!("Tab {} closed", idx)))
                        } else {
                            Ok(ToolResult::fail(format!("Tab index {} out of range", idx)))
                        }
                    } else {
                        let matching = targets.iter().find(|t| t.id == tid);
                        if let Some(t) = matching {
                            cdp_err!(cdp.close_target(&t.id).await);
                            Ok(ToolResult::ok(format!("Tab {} closed", tid)))
                        } else {
                            Ok(ToolResult::fail(format!("Tab ID '{}' not found", tid)))
                        }
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
                let tab_id = args.get("tab_id").and_then(|v| v.as_str()).unwrap_or("");
                let target = if let Ok(idx) = tab_id.parse::<usize>() {
                    if idx < targets.len() {
                        &targets[idx]
                    } else {
                        return Ok(ToolResult::fail(format!("Tab index {} out of range", idx)));
                    }
                } else {
                    match targets.iter().find(|t| t.id == tab_id) {
                        Some(t) => t,
                        None => {
                            return Ok(ToolResult::fail(format!("Tab ID '{}' not found", tab_id)));
                        }
                    }
                };
                let ws_url = target.web_socket_debugger_url.clone();
                cdp_err!(cdp.activate_target(&target.id).await);
                cdp.disconnect().await?;
                {
                    let mut guard = self.client.lock().await;
                    *guard = None;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                let new_cdp = CdpClient::new(&self.endpoint());
                cdp_err!(new_cdp.connect_to_target(&ws_url).await);
                cdp_err!(new_cdp.enable_page_domain().await);
                cdp_err!(new_cdp.enable_network().await);
                let mut guard = self.client.lock().await;
                *guard = Some(new_cdp);
                Ok(ToolResult::ok(format!("Switched to tab {}", target.id)))
            }
            "drag" => {
                let f = get_sel!(args);
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
            "snapshot_structured" => {
                let compact = args
                    .get("compact")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let depth = args.get("depth").and_then(|v| v.as_i64()).map(|d| d as i32);
                let result = cdp_err!(cdp.snapshot_structured(compact, depth).await);
                Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&result).unwrap_or_default(),
                ))
            }
            "query_selector" => {
                let s = req_sel!(args);
                let result = cdp_err!(cdp.query_selector_structured(s).await);
                Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&result).unwrap_or_default(),
                ))
            }
            "find_xpath" => {
                let xpath = args.get("xpath").and_then(|v| v.as_str()).unwrap_or("//*");
                let result = cdp_err!(cdp.find_by_xpath(xpath).await);
                Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&result).unwrap_or_default(),
                ))
            }
            "send_cdp" => {
                let method = args
                    .get("method")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Browser.getVersion");
                let params = match args.get("params") {
                    Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
                        Ok(v) => v,
                        Err(e) => {
                            return Ok(ToolResult::fail(format!("Invalid params JSON: {}", e)));
                        }
                    },
                    Some(v @ Value::Object(_)) => v.clone(),
                    Some(v) => {
                        return Ok(ToolResult::fail(format!(
                            "params must be a JSON object or JSON string, got {}",
                            v
                        )));
                    }
                    None => json!({}),
                };
                let result = cdp_err!(cdp.send_command(method, params).await);
                Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&result).unwrap_or_default(),
                ))
            }
            "accept_dialog" => {
                let prompt_text = args.get("text").and_then(|v| v.as_str());
                cdp_err!(cdp.handle_dialog(true, prompt_text).await);
                Ok(ToolResult::ok("Dialog accepted".to_string()))
            }
            "dismiss_dialog" => {
                cdp_err!(cdp.handle_dialog(false, None).await);
                Ok(ToolResult::ok("Dialog dismissed".to_string()))
            }
            "upload_file" => {
                let s = req_sel!(args);
                let raw_files: Vec<String> = args
                    .get("file_paths")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                if raw_files.is_empty() {
                    return Ok(ToolResult::fail(
                        "file_paths array required for upload_file",
                    ));
                }
                let files = match self.validate_upload_files(&raw_files) {
                    Ok(files) => files,
                    Err(e) => return Ok(ToolResult::fail(e.to_string())),
                };
                cdp_err!(cdp.set_file_input_files(s, &files).await);
                Ok(ToolResult::ok(format!(
                    "Uploaded {} file(s) to {}",
                    files.len(),
                    s
                )))
            }
            "console_start" => {
                cdp_err!(cdp.start_console_capture().await);
                Ok(ToolResult::ok("Console capture started".to_string()))
            }
            "console_get" => {
                let result = cdp_err!(cdp.get_console_log().await);
                Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&result).unwrap_or_default(),
                ))
            }
            "console_clear" => {
                cdp_err!(cdp.clear_console_log().await);
                Ok(ToolResult::ok("Console log cleared".to_string()))
            }
            "network_start" => {
                cdp_err!(cdp.start_network_capture().await);
                Ok(ToolResult::ok("Network capture started".to_string()))
            }
            "network_get" => {
                let result = cdp_err!(cdp.get_network_log().await);
                Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&result).unwrap_or_default(),
                ))
            }
            "network_clear" => {
                cdp_err!(cdp.clear_network_log().await);
                Ok(ToolResult::ok("Network log cleared".to_string()))
            }
            "switch_to_frame" => {
                let s = req_sel!(args);
                cdp_err!(cdp.switch_to_frame(s).await);
                Ok(ToolResult::ok(format!("Switched to frame {}", s)))
            }
            "default_content" => {
                cdp_err!(cdp.default_content().await);
                Ok(ToolResult::ok("Switched back to main document".to_string()))
            }
            "grant_permissions" => {
                let permissions: Vec<String> = args
                    .get("permissions")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                if permissions.is_empty() {
                    return Ok(ToolResult::fail("permissions array required"));
                }
                cdp_err!(cdp.grant_permissions(&permissions).await);
                Ok(ToolResult::ok(format!(
                    "Granted permissions: {}",
                    permissions.join(", ")
                )))
            }
            "set_user_agent" => {
                let ua = args
                    .get("user_agent")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                cdp_err!(cdp.set_user_agent(ua).await);
                Ok(ToolResult::ok(format!("User agent set to: {}", ua)))
            }
            "set_zoom" => {
                let zoom = args.get("zoom").and_then(|v| v.as_f64()).unwrap_or(1.0);
                cdp_err!(cdp.set_page_zoom(zoom).await);
                Ok(ToolResult::ok(format!("Page zoom set to {}", zoom)))
            }
            "set_network_conditions" => {
                let offline = false;
                let latency = args.get("latency").and_then(|v| v.as_f64());
                let dl = args.get("download_throughput").and_then(|v| v.as_f64());
                let ul = args.get("upload_throughput").and_then(|v| v.as_f64());
                cdp_err!(cdp.set_network_conditions(offline, latency, dl, ul).await);
                Ok(ToolResult::ok("Network conditions set".to_string()))
            }
            "set_download_behavior" => {
                let behavior = args
                    .get("behavior")
                    .and_then(|v| v.as_str())
                    .unwrap_or("allow");
                let download_path = match args.get("path").and_then(|v| v.as_str()) {
                    Some(path) if !path.trim().is_empty() => {
                        match self.resolve_workspace_output_path(Some(path), path.to_string()) {
                            Ok(path) => Some(path),
                            Err(e) => return Ok(ToolResult::fail(e.to_string())),
                        }
                    }
                    _ => None,
                };
                cdp_err!(
                    cdp.set_download_behavior(behavior, download_path.as_deref())
                        .await
                );
                Ok(ToolResult::ok(format!(
                    "Download behavior set to {}",
                    behavior
                )))
            }
            _ => Ok(ToolResult::fail(format!(
                "Action '{}' not supported via CDP. Available: navigate, click, dblclick, type, fill, scroll, hover, focus, check, uncheck, scrollintoview, screenshot, screenshot_element, pdf, read_page, read_dom, back, forward, refresh, get_url, get_title, get_text, get_html, get_value, get_attr, get_count, get_box, get_styles, is_visible, is_enabled, is_checked, eval, select_option, key_press, mouse_move, mouse_click, mouse_wheel, get_cookies, set_cookie, clear_cookies, storage_local, storage_session, wait, wait_text, wait_load, close, set_viewport, set_device, set_geo, clear_geo, set_offline, set_headers, set_media, tab_new, tab_list, tab_close, tab_switch, drag, snapshot, snapshot_structured, query_selector, find_xpath, send_cdp, accept_dialog, dismiss_dialog, upload_file, console_start, console_get, console_clear, network_start, network_get, network_clear, switch_to_frame, default_content, grant_permissions, set_user_agent, set_zoom, set_network_conditions, set_download_behavior",
                action
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Tool;

    #[test]
    fn browser_schema_exposes_status_action() {
        let tool = BrowserTool::new(".", &BrowserConfig::default());
        let schema: Value = serde_json::from_str(&tool.parameters_json()).unwrap();
        let actions = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");
        assert!(actions.iter().any(|v| v.as_str() == Some("status")));
    }
}
