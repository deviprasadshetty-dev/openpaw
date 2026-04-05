use super::{Tool, ToolContext, ToolResult, process_util};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

pub struct BrowserTool {
    pub workspace_dir: String,
}

impl BrowserTool {
    pub fn new(workspace_dir: impl Into<String>) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
        }
    }

    async fn run_cmd(&self, args: Vec<&str>) -> Result<ToolResult> {
        let is_headless = std::env::var("OPENPAW_BROWSER_HEADLESS")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false); // Default to headed (visible)

        #[cfg(windows)]
        let mut full_args = vec![
            "cmd.exe",
            "/c",
            "agent-browser",
            "--session-name",
            "openpaw",
        ];
        #[cfg(not(windows))]
        let mut full_args = vec!["agent-browser", "--session-name", "openpaw"];

        if !is_headless {
            full_args.push("--headed");
        }

        full_args.extend(args);

        let opts = process_util::RunOptions {
            timeout_ms: 60_000, // 60s timeout for browser actions
            ..Default::default()
        };

        let res = process_util::run(&full_args, opts).await?;

        if res.success {
            Ok(ToolResult::ok(res.stdout))
        } else {
            Ok(ToolResult::fail(format!(
                "Browser action failed (exit {}): {}\n{}",
                res.exit_code.unwrap_or(-1),
                res.stderr,
                res.stdout
            )))
        }
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Advanced browser automation using agent-browser. The tool manages its own browser internally; no external Chrome with --remote-debugging-port (CDP) is required. \
         Core workflow: 1) navigate -> 2) snapshot -i (get @e1 refs) -> 3) interact using refs -> 4) re-snapshot after navigation. \
         Key actions: navigate, snapshot (-i interactive, -c compact, -d depth, -s selector), click/fill/type/select/check, wait (element/text/url/networkidle), screenshot (--full, --annotate), \
         get (text/html/value/attr/title/url/count/box/styles), is (visible/enabled/checked), eval (JS with --stdin/-b for complex), find (semantic locators), \
         state (save/load/list/clear/clean), diff (snapshot/screenshot/url), storage (local/session), cookies, tabs, keyboard (type/inserttext), scroll, drag, upload, download, pdf, \
         record (start/stop/restart), network (route/unroute/requests), auth (save/login/list/show/delete), set (viewport/device/geo/offline/headers/media), \
         iOS gestures (tap/swipe), connect (--auto-connect, --cdp port). Use --json for structured output, --new-tab for clicks. Session persistence via --session-name."
    }

    fn parameters_json(&self) -> String {
        r#"{
          "type": "object",
          "properties": {
            "action": {
              "type": "string",
              "enum": ["navigate", "click", "dblclick", "type", "fill", "keyboard_type", "keyboard_insert", "scroll", "hover", "focus", "check", "uncheck", "scrollintoview", "upload", "download", "screenshot", "screenshot_element", "pdf", "read_page", "read_dom", "back", "forward", "refresh", "get_url", "get_title", "get_text", "get_html", "get_value", "get_attr", "get_count", "get_box", "get_styles", "is_visible", "is_enabled", "is_checked", "eval", "find", "select_option", "key_press", "mouse_move", "mouse_click", "mouse_wheel", "get_cookies", "set_cookie", "clear_cookies", "storage_local", "storage_session", "wait", "wait_text", "wait_url", "wait_load", "close", "set_viewport", "set_device", "set_geo", "set_offline", "set_headers", "set_media", "alert_accept", "alert_dismiss", "drag", "tab_new", "tab_list", "tab_close", "tab_switch", "window_new", "frame", "snapshot", "state_save", "state_load", "state_list", "state_clear", "state_clean", "diff_snapshot", "diff_screenshot", "diff_url", "record_start", "record_stop", "record_restart", "network_route", "network_unroute", "network_requests", "auth_save", "auth_login", "auth_list", "auth_show", "auth_delete", "tap", "swipe", "connect", "highlight", "inspect", "get_cdp_url"],
              "description": "Action to perform"
            },
            "url":       { "type": "string",  "description": "URL for navigate/tab_new/wait_url/diff_url" },
            "selector":  { "type": "string",  "description": "CSS selector or @eN element index" },
            "text":      { "type": "string",  "description": "Text to type/eval/wait for, or JS code for eval" },
            "tab_id":    { "type": "integer", "description": "ID or index of the tab to operate on" },
            "direction": { "type": "string",  "enum": ["up", "down", "left", "right"], "description": "Scroll direction" },
            "amount":     { "type": "integer", "description": "Scroll/wheel amount or snapshot depth" },
            "ms":        { "type": "integer", "description": "Wait duration in ms" },
            "key":       { "type": "string",  "description": "Key name for key_press" },
            "option":    { "type": "string",  "description": "Value or label for select_option" },
            "path":      { "type": "string",  "description": "File path for upload/pdf/screenshot/download/state" },
            "attribute": { "type": "string",  "description": "Attribute name for get_attr" },
            "device":    { "type": "string",  "description": "Device name for set_device" },
            "latitude":  { "type": "number", "description": "Latitude for set_geo" },
            "longitude": { "type": "number", "description": "Longitude for set_geo" },
            "cookie_name":   { "type": "string", "description": "Cookie/Storage/State name or auth profile name" },
            "cookie_value":  { "type": "string", "description": "Cookie/Storage value" },
            "x":           { "type": "integer", "description": "X coordinate for mouse" },
            "y":           { "type": "integer", "description": "Y coordinate for mouse" },
            "button":      { "type": "string", "enum": ["left", "right", "middle"], "description": "Mouse button" },
            "locator":     { "type": "string", "enum": ["role", "text", "label", "placeholder", "alt", "title", "testid", "first", "last", "nth"], "description": "Locator type for find" },
            "value":       { "type": "string", "description": "Value for locator" },
            "sub_action":  { "type": "string", "description": "Action to perform on found element (e.g. click, text)" },
            "load_state":  { "type": "string", "enum": ["load", "domcontentloaded", "networkidle"], "description": "Load state to wait for" },
            "headers":     { "type": "string", "description": "JSON string of headers for set_headers" },
            "media_scheme":{ "type": "string", "enum": ["dark", "light", "no-preference"], "description": "Color scheme for set_media" },
            "width":       { "type": "integer", "description": "Width for set_viewport" },
            "height":      { "type": "integer", "description": "Height for set_viewport" },
            "scale":       { "type": "integer", "description": "Device pixel ratio for set_viewport (retina)" },
            "to_selector": { "type": "string", "description": "Target selector for drag" },
            "baseline":    { "type": "string", "description": "Baseline file for diff comparison" },
            "url2":        { "type": "string", "description": "Second URL for diff_url" },
            "username":    { "type": "string", "description": "Username for auth_save" },
            "password":    { "type": "string", "description": "Password for auth_save" },
            "gesture":     { "type": "string", "enum": ["up", "down", "left", "right"], "description": "Swipe gesture direction" },
            "port":        { "type": "integer", "description": "CDP port for connect" },
            "json_output": { "type": "boolean", "description": "Return JSON output for snapshot/get commands" },
            "use_base64":  { "type": "boolean", "description": "Base64-encode JS for eval (-b flag), use when script contains quotes or special characters" },
            "full_page":   { "type": "boolean", "description": "Full page screenshot" },
            "annotate":    { "type": "boolean", "description": "Annotated screenshot with element labels" },
            "new_tab":     { "type": "boolean", "description": "Open click in new tab" },
            "compact":     { "type": "boolean", "description": "Compact snapshot output" },
            "scope":       { "type": "string", "description": "CSS selector to scope snapshot" },
            "depth":       { "type": "integer", "description": "Max depth for snapshot" },
            "days":        { "type": "integer", "description": "Days for state_clean (older than)" }
          },
          "required": ["action"]
        }"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");

        match action {
            "navigate" => {
                let url = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("about:blank");
                self.run_cmd(vec!["open", url]).await
            }
            "click" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let mut cmd = vec!["click", selector];
                if args
                    .get("new_tab")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    cmd.push("--new-tab");
                }
                self.run_cmd(cmd).await
            }
            "dblclick" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["dblclick", selector]).await
            }
            "type" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["type", selector, text]).await
            }
            "fill" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["fill", selector, text]).await
            }
            "keyboard_type" => {
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["keyboard", "type", text]).await
            }
            "keyboard_insert" => {
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["keyboard", "inserttext", text]).await
            }
            "scroll" => {
                let direction = args
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("down");
                let amount = args
                    .get("amount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(500)
                    .to_string();
                self.run_cmd(vec!["scroll", direction, &amount]).await
            }
            "hover" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["hover", selector]).await
            }
            "focus" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["focus", selector]).await
            }
            "check" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["check", selector]).await
            }
            "uncheck" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["uncheck", selector]).await
            }
            "scrollintoview" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["scrollintoview", selector]).await
            }
            "upload" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["upload", selector, path]).await
            }
            "download" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["download", selector, path]).await
            }
            "screenshot" => {
                let path_arg = args.get("path").and_then(|v| v.as_str());
                let path_str = if let Some(p) = path_arg {
                    p.to_string()
                } else {
                    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
                    let filename = format!("screenshot_{}.png", ts);
                    let path = Path::new(&self.workspace_dir)
                        .join("screenshots")
                        .join(&filename);
                    let _ = std::fs::create_dir_all(path.parent().unwrap());
                    path.to_string_lossy().to_string()
                };
                let mut cmd_args = vec!["screenshot", &path_str];
                if args
                    .get("full_page")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    cmd_args.push("--full");
                }
                if args
                    .get("annotate")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    cmd_args.push("--annotate");
                }
                self.run_cmd(cmd_args).await
            }
            "screenshot_element" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
                let filename = format!("element_{}.png", ts);
                let path = Path::new(&self.workspace_dir)
                    .join("screenshots")
                    .join(&filename);
                let _ = std::fs::create_dir_all(path.parent().unwrap());
                let path_str = path.to_string_lossy().to_string();
                if !selector.is_empty() {
                    let _ = self.run_cmd(vec!["highlight", selector]).await;
                }
                self.run_cmd(vec!["screenshot", &path_str]).await
            }
            "pdf" => {
                let path_arg = args.get("path").and_then(|v| v.as_str());
                let path_str = if let Some(p) = path_arg {
                    p.to_string()
                } else {
                    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
                    let filename = format!("page_{}.pdf", ts);
                    let path = Path::new(&self.workspace_dir)
                        .join("downloads")
                        .join(&filename);
                    let _ = std::fs::create_dir_all(path.parent().unwrap());
                    path.to_string_lossy().to_string()
                };
                self.run_cmd(vec!["pdf", &path_str]).await
            }
            "read_page" => self.run_cmd(vec!["snapshot", "-c"]).await,
            "read_dom" => self.run_cmd(vec!["snapshot", "-i"]).await,
            "back" => self.run_cmd(vec!["back"]).await,
            "forward" => self.run_cmd(vec!["forward"]).await,
            "refresh" => self.run_cmd(vec!["reload"]).await,
            "get_url" => self.run_cmd(vec!["get", "url"]).await,
            "get_title" => self.run_cmd(vec!["get", "title"]).await,
            "get_text" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["get", "text", s]).await
            }
            "get_html" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["get", "html", s]).await
            }
            "get_value" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["get", "value", s]).await
            }
            "get_attr" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let a = args.get("attribute").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["get", "attr", a, s]).await
            }
            "get_count" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["get", "count", s]).await
            }
            "get_box" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["get", "box", s]).await
            }
            "get_styles" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["get", "styles", s]).await
            }
            "is_visible" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["is", "visible", s]).await
            }
            "is_enabled" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["is", "enabled", s]).await
            }
            "is_checked" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["is", "checked", s]).await
            }
            "eval" => {
                let js_code = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                // use_base64: encode JS as base64 (-b flag) to avoid shell-escaping issues
                // when the script contains quotes, backticks, or other special characters.
                let use_base64 = args
                    .get("use_base64")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if use_base64 {
                    use base64::{Engine, engine::general_purpose::STANDARD};
                    let encoded = STANDARD.encode(js_code.as_bytes());
                    self.run_cmd(vec!["eval", "-b", &encoded]).await
                } else {
                    self.run_cmd(vec!["eval", js_code]).await
                }
            }
            "find" => {
                let l = args
                    .get("locator")
                    .and_then(|v| v.as_str())
                    .unwrap_or("role");
                let v = args.get("value").and_then(|v| v.as_str()).unwrap_or("");
                let sa = args
                    .get("sub_action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("click");
                let mut cmd = vec!["find", l, v, sa];
                if let Some(t) = args.get("text").and_then(|v| v.as_str()) {
                    cmd.push(t);
                }
                self.run_cmd(cmd).await
            }
            "select_option" => {
                let s = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let o = args.get("option").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["select", s, o]).await
            }
            "key_press" => {
                let k = args.get("key").and_then(|v| v.as_str()).unwrap_or("Enter");
                self.run_cmd(vec!["press", k]).await
            }
            "mouse_move" => {
                let x = args
                    .get("x")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    .to_string();
                let y = args
                    .get("y")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    .to_string();
                self.run_cmd(vec!["mouse", "move", &x, &y]).await
            }
            "mouse_click" => {
                let b = args
                    .get("button")
                    .and_then(|v| v.as_str())
                    .unwrap_or("left");
                self.run_cmd(vec!["mouse", "down", b]).await?;
                self.run_cmd(vec!["mouse", "up", b]).await
            }
            "mouse_wheel" => {
                let a = args
                    .get("amount")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(100)
                    .to_string();
                self.run_cmd(vec!["mouse", "wheel", &a]).await
            }
            "get_cookies" => self.run_cmd(vec!["cookies", "get"]).await,
            "set_cookie" => {
                let n = args
                    .get("cookie_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let v = args
                    .get("cookie_value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                self.run_cmd(vec!["cookies", "set", n, v]).await
            }
            "clear_cookies" => self.run_cmd(vec!["cookies", "clear"]).await,
            "storage_local" => {
                let mut cmd = vec!["storage", "local"];
                if let Some(n) = args.get("cookie_name").and_then(|v| v.as_str()) {
                    cmd.push(n);
                }
                if let Some(v) = args.get("cookie_value").and_then(|v| v.as_str()) {
                    cmd.push(v);
                }
                self.run_cmd(cmd).await
            }
            "storage_session" => {
                let mut cmd = vec!["storage", "session"];
                if let Some(n) = args.get("cookie_name").and_then(|v| v.as_str()) {
                    cmd.push(n);
                }
                if let Some(v) = args.get("cookie_value").and_then(|v| v.as_str()) {
                    cmd.push(v);
                }
                self.run_cmd(cmd).await
            }
            "wait" => {
                if let Some(ms) = args.get("ms").and_then(|v| v.as_u64()) {
                    self.run_cmd(vec!["wait", &ms.to_string()]).await
                } else if let Some(s) = args.get("selector").and_then(|v| v.as_str()) {
                    self.run_cmd(vec!["wait", s]).await
                } else {
                    self.run_cmd(vec!["wait", "1000"]).await
                }
            }
            "wait_text" => {
                let t = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["wait", "--text", t]).await
            }
            "wait_url" => {
                let u = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["wait", "--url", u]).await
            }
            "wait_load" => {
                let ls = args
                    .get("load_state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("networkidle");
                self.run_cmd(vec!["wait", "--load", ls]).await
            }
            "close" => self.run_cmd(vec!["close"]).await,
            "set_viewport" => {
                let w = args
                    .get("width")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1280)
                    .to_string();
                let h = args
                    .get("height")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(800)
                    .to_string();
                self.run_cmd(vec!["set", "viewport", &w, &h]).await
            }
            "set_device" => {
                let d = args.get("device").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["set", "device", d]).await
            }
            "set_geo" => {
                let lat = args
                    .get("latitude")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
                    .to_string();
                let lon = args
                    .get("longitude")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
                    .to_string();
                self.run_cmd(vec!["set", "geo", &lat, &lon]).await
            }
            "set_offline" => self.run_cmd(vec!["set", "offline", "on"]).await,
            "set_headers" => {
                let h = args.get("headers").and_then(|v| v.as_str()).unwrap_or("{}");
                self.run_cmd(vec!["set", "headers", h]).await
            }
            "set_media" => {
                let m = args
                    .get("media_scheme")
                    .and_then(|v| v.as_str())
                    .unwrap_or("dark");
                self.run_cmd(vec!["set", "media", m]).await
            }
            "alert_accept" => {
                let t = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if t.is_empty() {
                    self.run_cmd(vec!["dialog", "accept"]).await
                } else {
                    self.run_cmd(vec!["dialog", "accept", t]).await
                }
            }
            "alert_dismiss" => self.run_cmd(vec!["dialog", "dismiss"]).await,
            "drag" => {
                let f = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let t = args
                    .get("to_selector")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                self.run_cmd(vec!["drag", f, t]).await
            }
            "tab_new" => {
                let u = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("about:blank");
                self.run_cmd(vec!["tab", "new", u]).await
            }
            "tab_list" => self.run_cmd(vec!["tab", "list"]).await,
            "tab_close" => self.run_cmd(vec!["tab", "close"]).await,
            "tab_switch" => {
                let id = args
                    .get("tab_id")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    .to_string();
                self.run_cmd(vec!["tab", &id]).await
            }
            "window_new" => self.run_cmd(vec!["window", "new"]).await,
            "frame" => {
                let s = args
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .unwrap_or("main");
                self.run_cmd(vec!["frame", s]).await
            }
            // Snapshot with options
            "snapshot" => {
                // depth_str must outlive cmd; allocate before building cmd.
                let depth_str = args
                    .get("depth")
                    .and_then(|v| v.as_u64())
                    .map(|d| d.to_string());

                let is_compact = args
                    .get("compact")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let mut cmd = vec!["snapshot"];
                if args
                    .get("json_output")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    cmd.push("--json");
                }
                if is_compact {
                    cmd.push("-c");
                }
                if let Some(ref d) = depth_str {
                    cmd.push("-d");
                    cmd.push(d.as_str());
                }
                if let Some(scope) = args.get("scope").and_then(|v| v.as_str()) {
                    cmd.push("-s");
                    cmd.push(scope);
                }
                // Default to interactive when not compact
                if !is_compact {
                    cmd.push("-i");
                }
                self.run_cmd(cmd).await
            }
            // State management
            "state_save" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("./state.json");
                self.run_cmd(vec!["state", "save", path]).await
            }
            "state_load" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("./state.json");
                self.run_cmd(vec!["state", "load", path]).await
            }
            "state_list" => self.run_cmd(vec!["state", "list"]).await,
            "state_clear" => {
                let name = args
                    .get("cookie_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                self.run_cmd(vec!["state", "clear", name]).await
            }
            "state_clean" => {
                let days = args
                    .get("days")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(30)
                    .to_string();
                self.run_cmd(vec!["state", "clean", "--older-than", &days])
                    .await
            }
            // Diff commands
            "diff_snapshot" => {
                let mut cmd = vec!["diff", "snapshot"];
                if let Some(baseline) = args.get("baseline").and_then(|v| v.as_str()) {
                    cmd.push("--baseline");
                    cmd.push(baseline);
                }
                self.run_cmd(cmd).await
            }
            "diff_screenshot" => {
                let mut cmd = vec!["diff", "screenshot"];
                if let Some(baseline) = args.get("baseline").and_then(|v| v.as_str()) {
                    cmd.push("--baseline");
                    cmd.push(baseline);
                }
                self.run_cmd(cmd).await
            }
            "diff_url" => {
                let url1 = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
                let url2 = args.get("url2").and_then(|v| v.as_str()).unwrap_or("");
                let mut cmd = vec!["diff", "url", url1, url2];
                if let Some(load_state) = args.get("load_state").and_then(|v| v.as_str()) {
                    cmd.push("--wait-until");
                    cmd.push(load_state);
                }
                if let Some(selector) = args.get("scope").and_then(|v| v.as_str()) {
                    cmd.push("--selector");
                    cmd.push(selector);
                }
                self.run_cmd(cmd).await
            }
            // Video recording
            "record_start" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("./recording.webm");
                self.run_cmd(vec!["record", "start", path]).await
            }
            "record_stop" => self.run_cmd(vec!["record", "stop"]).await,
            "record_restart" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("./recording.webm");
                self.run_cmd(vec!["record", "restart", path]).await
            }
            // Network controls
            "network_route" => {
                let url_pattern = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
                let body = args.get("text").and_then(|v| v.as_str());
                let mut cmd = vec!["network", "route", url_pattern];
                if let Some(b) = body {
                    cmd.push("--body");
                    cmd.push(b);
                } else {
                    cmd.push("--abort");
                }
                self.run_cmd(cmd).await
            }
            "network_unroute" => {
                let url_pattern = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["network", "unroute", url_pattern]).await
            }
            "network_requests" => {
                let clear = args.get("cookie_name").and_then(|v| v.as_str()) == Some("clear");
                if clear {
                    self.run_cmd(vec!["network", "requests", "--clear"]).await
                } else {
                    self.run_cmd(vec!["network", "requests"]).await
                }
            }
            // Auth vault
            "auth_save" => {
                let name = args
                    .get("cookie_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
                let username = args.get("username").and_then(|v| v.as_str()).unwrap_or("");
                let password = args.get("password").and_then(|v| v.as_str()).unwrap_or("");
                let mut cmd = vec!["auth", "save", name, "--url", url, "--username", username];
                if !password.is_empty() {
                    cmd.push("--password");
                    cmd.push(password);
                }
                self.run_cmd(cmd).await
            }
            "auth_login" => {
                let name = args
                    .get("cookie_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                self.run_cmd(vec!["auth", "login", name]).await
            }
            "auth_list" => self.run_cmd(vec!["auth", "list"]).await,
            "auth_show" => {
                let name = args
                    .get("cookie_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                self.run_cmd(vec!["auth", "show", name]).await
            }
            "auth_delete" => {
                let name = args
                    .get("cookie_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                self.run_cmd(vec!["auth", "delete", name]).await
            }
            // iOS gestures
            "tap" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["tap", selector]).await
            }
            "swipe" => {
                let gesture = args.get("gesture").and_then(|v| v.as_str()).unwrap_or("up");
                self.run_cmd(vec!["swipe", gesture]).await
            }
            // Connect to existing browser
            "connect" => {
                if let Some(port) = args.get("port").and_then(|v| v.as_u64()) {
                    self.run_cmd(vec!["connect", &port.to_string()]).await
                } else if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
                    self.run_cmd(vec!["connect", url]).await
                } else {
                    self.run_cmd(vec!["connect", "9222"]).await
                }
            }
            // Debug utilities
            "highlight" => {
                let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                self.run_cmd(vec!["highlight", selector]).await
            }
            "inspect" => self.run_cmd(vec!["inspect"]).await,
            "get_cdp_url" => self.run_cmd(vec!["get", "cdp-url"]).await,
            _ => Ok(ToolResult::fail(format!(
                "Action '{}' not supported",
                action
            ))),
        }
    }
}
