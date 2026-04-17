use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, Clone)]
pub struct CdpTarget {
    pub id: String,
    pub target_type: String,
    pub title: String,
    pub url: String,
    pub web_socket_debugger_url: String,
}

#[derive(Debug, Deserialize)]
struct CdpListTarget {
    id: String,
    #[serde(rename = "type")]
    target_type: String,
    title: String,
    url: String,
    // Optional: Edge and Chrome omit this field for non-page targets
    // (service workers, browser internal tabs, etc.).  Making it required
    // causes the entire Vec deserialization to fail if any entry lacks it.
    #[serde(rename = "webSocketDebuggerUrl", default)]
    web_socket_debugger_url: Option<String>,
}

#[derive(Clone)]
pub struct CdpClient {
    ws: Arc<Mutex<Option<WsStream>>>,
    msg_id: Arc<AtomicU32>,
    endpoint: Arc<String>,
}

impl CdpClient {
    pub fn new(endpoint: &str) -> Self {
        Self {
            ws: Arc::new(Mutex::new(None)),
            msg_id: Arc::new(AtomicU32::new(1)),
            endpoint: Arc::new(endpoint.to_string()),
        }
    }

    pub async fn connect(&self) -> Result<()> {
        let ws_url = self.resolve_ws_url().await?;
        let (stream, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .context("Failed to connect to Chrome CDP WebSocket")?;
        let mut guard = self.ws.lock().await;
        *guard = Some(stream);
        Ok(())
    }

    pub async fn ensure_connected(&self) -> Result<()> {
        {
            let guard = self.ws.lock().await;
            if guard.is_some() {
                return Ok(());
            }
        }
        self.connect().await
    }

    async fn resolve_ws_url(&self) -> Result<String> {
        let http_url = format!("http://{}/json", self.endpoint);
        let resp = reqwest::get(&http_url)
            .await
            .context("Failed to reach Chrome DevTools HTTP endpoint — make sure Chrome is running with --remote-debugging-port enabled")?;

        let body = resp.text().await.context("Failed to read CDP response body")?;

        let targets: Vec<CdpListTarget> = serde_json::from_str(&body).with_context(|| {
            // Show the first 200 chars so the user can see if it returned HTML instead of JSON
            let preview = if body.len() > 200 { &body[..200] } else { &body };
            format!(
                "Failed to parse Chrome DevTools target list. \
                 Make sure the browser is running with --remote-debugging-port={}. \
                 Response preview: {}",
                self.endpoint
                    .split(':')
                    .last()
                    .unwrap_or("9222"),
                preview
            )
        })?;

        // Chrome/Edge may still be initializing — also, only page targets
        // have a webSocketDebuggerUrl; other types (worker, browser, etc.) may not.
        let page = targets
            .iter()
            .find(|t| t.target_type == "page" && t.web_socket_debugger_url.is_some())
            .ok_or_else(|| {
                let page_count = targets.iter().filter(|t| t.target_type == "page").count();
                anyhow!(
                    "No debuggable 'page' target found in Chrome DevTools \
                     ({} total targets, {} page-type). \
                     Chrome may still be starting up — will retry.",
                    targets.len(),
                    page_count
                )
            })?;

        Ok(page.web_socket_debugger_url.clone().unwrap())
    }

    pub async fn discover_targets(&self) -> Result<Vec<CdpTarget>> {
        let http_url = format!("http://{}/json", self.endpoint);
        let body = reqwest::get(&http_url).await?.text().await?;
        let targets: Vec<CdpListTarget> = serde_json::from_str(&body)?;
        Ok(targets
            .into_iter()
            // Only expose targets that actually have a WS debug URL
            .filter_map(|t| {
                t.web_socket_debugger_url.map(|ws| CdpTarget {
                    id: t.id,
                    target_type: t.target_type,
                    title: t.title,
                    url: t.url,
                    web_socket_debugger_url: ws,
                })
            })
            .collect())
    }

    pub async fn send_command(&self, method: &str, params: Value) -> Result<Value> {
        self.ensure_connected().await?;
        let id = self.msg_id.fetch_add(1, Ordering::SeqCst);

        let msg = json!({
            "id": id,
            "method": method,
            "params": params,
        });

        let msg_str = serde_json::to_string(&msg)?;
        let ws_msg = Message::Text(msg_str.into());

        {
            let mut guard = self.ws.lock().await;
            let stream = guard
                .as_mut()
                .ok_or_else(|| anyhow!("WebSocket not connected"))?;
            stream.send(ws_msg).await?;
        }

        let expected_id = id;
        let mut guard = self.ws.lock().await;
        let stream = guard
            .as_mut()
            .ok_or_else(|| anyhow!("WebSocket not connected"))?;

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(30);

        loop {
            let next = tokio::time::timeout_at(deadline, stream.next()).await;
            match next {
                Ok(Some(Ok(Message::Text(text)))) => {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                        if parsed.get("id").and_then(|v| v.as_u64()) == Some(expected_id as u64) {
                            if let Some(error) = parsed.get("error") {
                                let err_msg = error
                                    .get("message")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown CDP error");
                                let err_code = error
                                    .get("code")
                                    .and_then(|v| v.as_i64())
                                    .unwrap_or(-1);
                                return Err(anyhow!(
                                    "CDP error {}: {}",
                                    err_code,
                                    err_msg
                                ));
                            }
                            return Ok(parsed
                                .get("result")
                                .cloned()
                                .unwrap_or(json!({})));
                        }
                    }
                }
                Ok(Some(Ok(Message::Ping(data)))) => {
                    let _ = stream.send(Message::Pong(data)).await;
                }
                Ok(Some(Ok(Message::Close(_)))) => {
                    *guard = None;
                    return Err(anyhow!("WebSocket connection closed by Chrome"));
                }
                Ok(Some(Err(e))) => {
                    return Err(anyhow!("WebSocket error: {}", e));
                }
                Ok(None) => {
                    *guard = None;
                    return Err(anyhow!("WebSocket stream ended"));
                }
                Err(_) => {
                    return Err(anyhow!(
                        "Timeout waiting for CDP response (method: {})",
                        method
                    ));
                }
                _ => {}
            }
        }
    }

    pub async fn disconnect(&self) -> Result<()> {
        let mut guard = self.ws.lock().await;
        if let Some(mut stream) = guard.take() {
            let _ = stream.close(None).await;
        }
        Ok(())
    }

    // ── Page domain ────────────────────────────────────────────

    pub async fn navigate(&self, url: &str) -> Result<Value> {
        self.send_command("Page.navigate", json!({ "url": url }))
            .await
    }

    pub async fn get_current_url(&self) -> Result<String> {
        let result = self
            .send_command("Runtime.evaluate", json!({ "expression": "window.location.href" }))
            .await?;
        result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Failed to get current URL"))
    }

    pub async fn get_title(&self) -> Result<String> {
        let result = self
            .send_command("Runtime.evaluate", json!({ "expression": "document.title" }))
            .await?;
        result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Failed to get page title"))
    }

    // ── DOM interaction ────────────────────────────────────────

    pub async fn click(&self, selector: &str) -> Result<Value> {
        let expr = format!(
            r#"
            (function() {{
                const el = document.querySelector({});
                if (!el) throw new Error('Element not found: ' + {});
                el.scrollIntoView({{block: 'center'}});
                el.click();
                return true;
            }})()
            "#,
            serde_json::to_string(selector)?,
            serde_json::to_string(selector)?
        );
        self.send_command("Runtime.evaluate", json!({ "expression": expr, "awaitPromise": true }))
            .await
    }

    pub async fn dblclick(&self, selector: &str) -> Result<Value> {
        let expr = format!(
            r#"
            (function() {{
                const el = document.querySelector({});
                if (!el) throw new Error('Element not found: ' + {});
                el.scrollIntoView({{block: 'center'}});
                const evt = new MouseEvent('dblclick', {{bubbles: true}});
                el.dispatchEvent(evt);
                return true;
            }})()
            "#,
            serde_json::to_string(selector)?,
            serde_json::to_string(selector)?
        );
        self.send_command("Runtime.evaluate", json!({ "expression": expr, "awaitPromise": true }))
            .await
    }

    pub async fn fill(&self, selector: &str, value: &str) -> Result<Value> {
        let expr = format!(
            r#"
            (function() {{
                const el = document.querySelector({0});
                if (!el) throw new Error('Element not found: ' + {0});
                el.focus();
                el.value = {1};
                el.dispatchEvent(new Event('input', {{bubbles: true}}));
                el.dispatchEvent(new Event('change', {{bubbles: true}}));
                return true;
            }})()
            "#,
            serde_json::to_string(selector)?,
            serde_json::to_string(value)?
        );
        self.send_command("Runtime.evaluate", json!({ "expression": expr }))
            .await
    }

    pub async fn type_text(&self, selector: &str, text: &str) -> Result<Value> {
        let expr = format!(
            r#"
            (function() {{
                const el = document.querySelector({0});
                if (!el) throw new Error('Element not found');
                el.focus();
                el.value = {1};
                el.dispatchEvent(new Event('input', {{bubbles: true}}));
                el.dispatchEvent(new Event('change', {{bubbles: true}}));
                return true;
            }})()
            "#,
            serde_json::to_string(selector)?,
            serde_json::to_string(text)?
        );
        self.send_command("Runtime.evaluate", json!({ "expression": expr }))
            .await
    }

    pub async fn keyboard_send_text(&self, text: &str) -> Result<Value> {
        self.send_command("Input.insertText", json!({ "text": text }))
            .await
    }

    pub async fn key_press(&self, key: &str, modifiers: Option<i32>) -> Result<Value> {
        let mut params = json!({
            "type": "keyDown",
            "key": key,
        });
        if let Some(m) = modifiers {
            params["modifiers"] = json!(m);
        }
        self.send_command("Input.dispatchKeyEvent", params).await?;
        let mut up_params = json!({
            "type": "keyUp",
            "key": key,
        });
        if let Some(m) = modifiers {
            up_params["modifiers"] = json!(m);
        }
        self.send_command("Input.dispatchKeyEvent", up_params).await
    }

    // ── Mouse actions ──────────────────────────────────────────

    pub async fn mouse_move(&self, x: f64, y: f64) -> Result<Value> {
        self.send_command(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseMoved",
                "x": x,
                "y": y,
            }),
        )
        .await
    }

    pub async fn mouse_click(&self, x: f64, y: f64, button: &str, click_count: i32) -> Result<Value> {
        self.send_command(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mousePressed",
                "x": x,
                "y": y,
                "button": button,
                "clickCount": click_count,
            }),
        )
        .await?;
        self.send_command(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseReleased",
                "x": x,
                "y": y,
                "button": button,
                "clickCount": click_count,
            }),
        )
        .await
    }

    pub async fn mouse_wheel(&self, x: f64, y: f64, delta_x: f64, delta_y: f64) -> Result<Value> {
        self.send_command(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseWheel",
                "x": x,
                "y": y,
                "deltaX": delta_x,
                "deltaY": delta_y,
            }),
        )
        .await
    }

    // ── Input.dispatchKeyEvent helpers ─────────────────────────

    pub async fn key_down(&self, key: &str, code: &str, modifiers: Option<i32>) -> Result<Value> {
        let mut params = json!({
            "type": "keyDown",
            "key": key,
            "code": code,
            "windowsVirtualKeyCode": key_to_vk(key),
        });
        if let Some(m) = modifiers {
            params["modifiers"] = json!(m);
        }
        self.send_command("Input.dispatchKeyEvent", params).await
    }

    pub async fn key_up(&self, key: &str, code: &str, modifiers: Option<i32>) -> Result<Value> {
        let mut params = json!({
            "type": "keyUp",
            "key": key,
            "code": code,
            "windowsVirtualKeyCode": key_to_vk(key),
        });
        if let Some(m) = modifiers {
            params["modifiers"] = json!(m);
        }
        self.send_command("Input.dispatchKeyEvent", params).await
    }

    // ── JavaScript evaluation ──────────────────────────────────

    pub async fn evaluate(&self, expression: &str, await_promise: bool) -> Result<Value> {
        self.send_command(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "awaitPromise": await_promise,
                "returnByValue": true,
            }),
        )
        .await
    }

    // ── Page snapshot & screenshot ─────────────────────────────

    pub async fn snapshot_dom(&self, depth: Option<i32>) -> Result<Value> {
        let doc = self
            .send_command("DOM.getDocument", json!({ "depth": depth.unwrap_or(-1) }))
            .await?;
        Ok(doc)
    }

    pub async fn screenshot(&self, format: &str, quality: Option<i32>) -> Result<Value> {
        let mut params = json!({
            "format": format,
        });
        if let Some(q) = quality {
            params["quality"] = json!(q);
        }
        self.send_command("Page.captureScreenshot", params).await
    }

    pub async fn screenshot_element(
        &self,
        selector: &str,
        format: &str,
    ) -> Result<Value> {
        let clip_expr = format!(
            r#"
            (function() {{
                const el = document.querySelector({});
                if (!el) return null;
                const r = el.getBoundingClientRect();
                return {{ x: r.x, y: r.y, width: r.width, height: r.height, scale: 1 }};
            }})()
            "#,
            serde_json::to_string(selector)?
        );
        let clip = self
            .evaluate(&clip_expr, false)
            .await?;
        let clip_val = clip
            .get("result")
            .and_then(|r| r.get("value"))
            .ok_or_else(|| anyhow!("Failed to get element bounds"))?;

        self.send_command(
            "Page.captureScreenshot",
            json!({
                "format": format,
                "clip": clip_val,
            }),
        )
        .await
    }

    // ── Navigation ─────────────────────────────────────────────

    pub async fn go_back(&self) -> Result<Value> {
        self.send_command("Page.navigateToHistoryEntry", json!({ "entryId": -1 }))
            .await
    }

    pub async fn go_forward(&self) -> Result<Value> {
        self.send_command("Page.navigateToHistoryEntry", json!({ "entryId": 1 }))
            .await
    }

    pub async fn reload(&self) -> Result<Value> {
        self.send_command("Page.reload", json!({})).await
    }

    // ── Cookies ────────────────────────────────────────────────

    pub async fn get_cookies(&self) -> Result<Value> {
        self.send_command("Network.getCookies", json!({})).await
    }

    pub async fn set_cookie(&self, name: &str, value: &str, domain: Option<&str>, path: Option<&str>) -> Result<Value> {
        let mut params = json!({
            "name": name,
            "value": value,
        });
        if let Some(d) = domain {
            params["domain"] = json!(d);
        }
        if let Some(p) = path {
            params["path"] = json!(p);
        }
        self.send_command("Network.setCookie", params).await
    }

    pub async fn clear_cookies(&self) -> Result<Value> {
        self.send_command("Network.clearBrowserCookies", json!({}))
            .await
    }

    // ── Storage ────────────────────────────────────────────────

    pub async fn get_local_storage(&self, key: Option<&str>) -> Result<Value> {
        let origin = self.get_current_url().await.unwrap_or_default();
        if let Some(k) = key {
            self.send_command(
                "DOMStorage.getDOMStorageItem",
                json!({
                    "storageId": { "securityOrigin": origin, "isLocalStorage": true },
                    "key": k,
                }),
            )
            .await
        } else {
            self.send_command(
                "DOMStorage.getDOMStorageItems",
                json!({
                    "storageId": { "securityOrigin": origin, "isLocalStorage": true },
                }),
            )
            .await
        }
    }

    pub async fn set_local_storage(&self, key: &str, value: &str) -> Result<Value> {
        let origin = self.get_current_url().await.unwrap_or_default();
        self.send_command(
            "DOMStorage.setDOMStorageItem",
            json!({
                "storageId": { "securityOrigin": origin, "isLocalStorage": true },
                "key": key,
                "value": value,
            }),
        )
        .await
    }

    // ── Tab management ─────────────────────────────────────────

    pub async fn list_targets(&self) -> Result<Vec<CdpTarget>> {
        self.discover_targets().await
    }

    pub async fn create_target(&self, url: &str) -> Result<String> {
        let result = self
            .send_command(
                "Target.createTarget",
                json!({ "url": url }),
            )
            .await?;
        result
            .get("targetId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Failed to create new tab"))
    }

    pub async fn close_target(&self, target_id: &str) -> Result<Value> {
        self.send_command("Target.closeTarget", json!({ "targetId": target_id }))
            .await
    }

    pub async fn activate_target(&self, target_id: &str) -> Result<Value> {
        self.send_command("Target.activateTarget", json!({ "targetId": target_id }))
            .await
    }

    // ── Viewport ───────────────────────────────────────────────

    pub async fn set_viewport(&self, width: i64, height: i64, device_scale_factor: Option<f64>) -> Result<Value> {
        let mut params = json!({
            "width": width,
            "height": height,
        });
        if let Some(dsf) = device_scale_factor {
            params["deviceScaleFactor"] = json!(dsf);
        }
        self.send_command("Emulation.setDeviceMetricsOverride", params)
            .await
    }

    pub async fn set_geolocation(&self, latitude: f64, longitude: f64) -> Result<Value> {
        self.send_command(
            "Emulation.setGeolocationOverride",
            json!({
                "latitude": latitude,
                "longitude": longitude,
                "accuracy": 100,
            }),
        )
        .await
    }

    pub async fn set_network_offline(&self, offline: bool) -> Result<Value> {
        self.send_command(
            "Network.emulateNetworkConditions",
            json!({
                "offline": offline,
                "latency": 0,
                "downloadThroughput": if offline { -1 } else { -1 },
                "uploadThroughput": if offline { -1 } else { -1 },
            }),
        )
        .await
    }

    pub async fn set_extra_http_headers(&self, headers: Value) -> Result<Value> {
        self.send_command("Network.setExtraHTTPHeaders", json!({ "headers": headers }))
            .await
    }

    pub async fn set_color_scheme(&self, scheme: &str) -> Result<Value> {
        let prefers = match scheme {
            "dark" => json!({ "prefersColorScheme": "dark" }),
            "light" => json!({ "prefersColorScheme": "light" }),
            _ => json!({}),
        };
        self.send_command("Emulation.setEmulatedMedia", prefers).await
    }

    // ── PDF ─────────────────────────────────────────────────────

    pub async fn print_pdf(&self) -> Result<Value> {
        self.send_command("Page.printToPDF", json!({}))
            .await
    }

    // ── Select option ──────────────────────────────────────────

    pub async fn select_option(&self, selector: &str, value: &str) -> Result<Value> {
        let expr = format!(
            r#"
            (function() {{
                const sel = document.querySelector({});
                if (!sel) throw new Error('Select not found');
                sel.value = {};
                sel.dispatchEvent(new Event('change', {{bubbles: true}}));
                return true;
            }})()
            "#,
            serde_json::to_string(selector)?,
            serde_json::to_string(value)?
        );
        self.send_command("Runtime.evaluate", json!({ "expression": expr }))
            .await
    }

    // ── Checkbox ───────────────────────────────────────────────

    pub async fn check(&self, selector: &str) -> Result<Value> {
        let expr = format!(
            r#"
            (function() {{
                const el = document.querySelector({});
                if (!el) throw new Error('Element not found');
                el.checked = true;
                el.dispatchEvent(new Event('change', {{bubbles: true}}));
                return true;
            }})()
            "#,
            serde_json::to_string(selector)?
        );
        self.send_command("Runtime.evaluate", json!({ "expression": expr }))
            .await
    }

    pub async fn uncheck(&self, selector: &str) -> Result<Value> {
        let expr = format!(
            r#"
            (function() {{
                const el = document.querySelector({});
                if (!el) throw new Error('Element not found');
                el.checked = false;
                el.dispatchEvent(new Event('change', {{bubbles: true}}));
                return true;
            }})()
            "#,
            serde_json::to_string(selector)?
        );
        self.send_command("Runtime.evaluate", json!({ "expression": expr }))
            .await
    }

    // ── Visibility / state checks ──────────────────────────────

    pub async fn is_visible(&self, selector: &str) -> Result<bool> {
        let expr = format!(
            r#"
            (function() {{
                const el = document.querySelector({});
                if (!el) return false;
                const rect = el.getBoundingClientRect();
                return rect.width > 0 && rect.height > 0 &&
                    window.getComputedStyle(el).visibility !== 'hidden' &&
                    window.getComputedStyle(el).display !== 'none';
            }})()
            "#,
            serde_json::to_string(selector)?
        );
        let result = self.evaluate(&expr, false).await?;
        result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_bool())
            .ok_or_else(|| anyhow!("Failed to check visibility"))
    }

    pub async fn is_enabled(&self, selector: &str) -> Result<bool> {
        let expr = format!(
            r#"(function() {{ const el = document.querySelector({}); return el ? !el.disabled : false; }})()"#,
            serde_json::to_string(selector)?
        );
        let result = self.evaluate(&expr, false).await?;
        result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_bool())
            .ok_or_else(|| anyhow!("Failed to check enabled state"))
    }

    pub async fn is_checked(&self, selector: &str) -> Result<bool> {
        let expr = format!(
            r#"(function() {{ const el = document.querySelector({}); return el ? el.checked : false; }})()"#,
            serde_json::to_string(selector)?
        );
        let result = self.evaluate(&expr, false).await?;
        result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_bool())
            .ok_or_else(|| anyhow!("Failed to check checked state"))
    }

    // ── Get DOM info ────────────────────────────────────────────

    pub async fn get_text(&self, selector: &str) -> Result<String> {
        let expr = format!(
            r#"(function() {{ const el = document.querySelector({}); return el ? el.innerText : ''; }})()"#,
            serde_json::to_string(selector)?
        );
        let result = self.evaluate(&expr, false).await?;
        result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Failed to get text content"))
    }

    pub async fn get_html(&self, selector: &str) -> Result<String> {
        let expr = format!(
            r#"(function() {{ const el = document.querySelector({}); return el ? el.outerHTML : ''; }})()"#,
            serde_json::to_string(selector)?
        );
        let result = self.evaluate(&expr, false).await?;
        result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Failed to get HTML"))
    }

    pub async fn get_value(&self, selector: &str) -> Result<String> {
        let expr = format!(
            r#"(function() {{ const el = document.querySelector({}); return el ? el.value : ''; }})()"#,
            serde_json::to_string(selector)?
        );
        let result = self.evaluate(&expr, false).await?;
        result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Failed to get value"))
    }

    pub async fn get_attribute(&self, selector: &str, attribute: &str) -> Result<String> {
        let expr = format!(
            r#"(function() {{ const el = document.querySelector({}); return el ? el.getAttribute({}) : ''; }})()"#,
            serde_json::to_string(selector)?,
            serde_json::to_string(attribute)?
        );
        let result = self.evaluate(&expr, false).await?;
        result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Failed to get attribute"))
    }

    pub async fn get_count(&self, selector: &str) -> Result<i64> {
        let expr = format!(
            r#"document.querySelectorAll({}).length"#,
            serde_json::to_string(selector)?
        );
        let result = self.evaluate(&expr, false).await?;
        result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow!("Failed to get element count"))
    }

    pub async fn get_bounding_box(&self, selector: &str) -> Result<Value> {
        let expr = format!(
            r#"(function() {{ const el = document.querySelector({}); if (!el) return null; const r = el.getBoundingClientRect(); return {{x:r.x,y:r.y,width:r.width,height:r.height}}; }})()"#,
            serde_json::to_string(selector)?
        );
        let result = self.evaluate(&expr, false).await?;
        result
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .ok_or_else(|| anyhow!("Failed to get bounding box"))
    }

    pub async fn get_computed_styles(&self, selector: &str) -> Result<Value> {
        let expr = format!(
            r#"JSON.stringify(Object.fromEntries(Array.from(window.getComputedStyle(document.querySelector({}))).map(p=>[p,window.getComputedStyle(document.querySelector({})).getPropertyValue(p)])))"#,
            serde_json::to_string(selector)?,
            serde_json::to_string(selector)?
        );
        let result = self.evaluate(&expr, false).await?;
        let styles_str = result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("{}");
        serde_json::from_str(styles_str).map_err(|e| anyhow!("Failed to parse styles: {}", e))
    }

    // ── Hover (mouse move + dispatch mouseover) ────────────────

    pub async fn hover(&self, selector: &str) -> Result<Value> {
        let box_result = self.get_bounding_box(selector).await?;
        let x = box_result
            .get("x")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
            + box_result
                .get("width")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                / 2.0;
        let y = box_result
            .get("y")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
            + box_result
                .get("height")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                / 2.0;

        self.mouse_move(x, y).await?;
        self.send_command(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseMoved",
                "x": x,
                "y": y,
            }),
        )
        .await
    }

    // ── Focus ──────────────────────────────────────────────────

    pub async fn focus(&self, selector: &str) -> Result<Value> {
        let expr = format!(
            r#"document.querySelector({}).focus()"#,
            serde_json::to_string(selector)?
        );
        self.evaluate(&expr, false).await
    }

    // ── Scroll ─────────────────────────────────────────────────

    pub async fn scroll(&self, direction: &str, amount: i64) -> Result<Value> {
        let (dx, dy) = match direction {
            "up" => (0, -amount),
            "down" => (0, amount),
            "left" => (-amount, 0),
            "right" => (amount, 0),
            _ => (0, amount),
        };
        let expr = format!("window.scrollBy({}, {})", dx, dy);
        self.evaluate(&expr, false).await
    }

    pub async fn scroll_into_view(&self, selector: &str) -> Result<Value> {
        let expr = format!(
            r#"document.querySelector({}).scrollIntoView({{block:'center'}})"#,
            serde_json::to_string(selector)?
        );
        self.evaluate(&expr, false).await
    }

    // ── Drag ───────────────────────────────────────────────────

    pub async fn drag(&self, from_selector: &str, to_selector: &str) -> Result<Value> {
        let from_box = self.get_bounding_box(from_selector).await?;
        let to_box = self.get_bounding_box(to_selector).await?;

        let from_x = from_box.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0)
            + from_box
                .get("width")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                / 2.0;
        let from_y = from_box.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0)
            + from_box
                .get("height")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                / 2.0;
        let to_x = to_box.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0)
            + to_box
                .get("width")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                / 2.0;
        let to_y = to_box.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0)
            + to_box
                .get("height")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                / 2.0;

        self.mouse_move(from_x, from_y).await?;
        self.mouse_click(from_x, from_y, "left", 1).await?;
        self.mouse_move(to_x, to_y).await?;

        let expr = format!(
            r#"(function() {{
                const from = document.querySelector({from_s});
                const to = document.querySelector({to_s});
                if (from && to) {{
                    from.dispatchEvent(new DragEvent('dragend', {{bubbles: true}}));
                    return true;
                }}
                return false;
            }})()"#,
            from_s = serde_json::to_string(from_selector)?,
            to_s = serde_json::to_string(to_selector)?
        );
        self.evaluate(&expr, false).await
    }

    // ── Wait / readiness ────────────────────────────────────────

    pub async fn wait_for_selector(&self, selector: &str, timeout_ms: u64) -> Result<Value> {
        let expr = format!(
            r#"
            new Promise((resolve, reject) => {{
                const timeout = setTimeout(() => reject(new Error('Timeout waiting for selector')), {timeout});
                if (document.querySelector({selector})) {{
                    clearTimeout(timeout);
                    resolve(true);
                    return;
                }}
                const observer = new MutationObserver(() => {{
                    if (document.querySelector({selector})) {{
                        clearTimeout(timeout);
                        observer.disconnect();
                        resolve(true);
                    }}
                }});
                observer.observe(document.body, {{childList: true, subtree: true}});
            }})
            "#,
            timeout = timeout_ms,
            selector = serde_json::to_string(selector)?
        );
        self.evaluate(&expr, true).await
    }

    pub async fn wait_for_text(&self, text: &str, timeout_ms: u64) -> Result<Value> {
        let expr = format!(
            r#"
            new Promise((resolve, reject) => {{
                const timeout = setTimeout(() => reject(new Error('Timeout waiting for text')), {timeout});
                if (document.body.innerText.includes({text})) {{
                    clearTimeout(timeout);
                    resolve(true);
                    return;
                }}
                const observer = new MutationObserver(() => {{
                    if (document.body.innerText.includes({text})) {{
                        clearTimeout(timeout);
                        observer.disconnect();
                        resolve(true);
                    }}
                }});
                observer.observe(document.body, {{childList: true, subtree: true}});
            }})
            "#,
            timeout = timeout_ms,
            text = serde_json::to_string(text)?
        );
        self.evaluate(&expr, true).await
    }

    pub async fn wait_for_load(&self, load_state: &str) -> Result<Value> {
        let expr = match load_state {
            "load" => "document.readyState === 'complete'".to_string(),
            "domcontentloaded" => "document.readyState !== 'loading'".to_string(),
            _ => "document.readyState === 'complete'".to_string(),
        };
        let check = format!(
            r#"
            new Promise(resolve => {{
                if ({}) {{ resolve(true); return; }}
                document.addEventListener('readystatechange', () => {{ if ({}) resolve(true); }});
            }})
            "#,
            expr, expr
        );
        self.evaluate(&check, true).await
    }

    // ── Dialog handling ────────────────────────────────────────

    pub async fn enable_page_domain(&self) -> Result<Value> {
        self.send_command("Page.enable", json!({})).await
    }

    // ── Snapshot (compact text-based DOM representation) ────────

    pub async fn snapshot_text(&self, compact: bool, max_depth: Option<i32>) -> Result<String> {
        let depth_expr = max_depth.map(|d| d.to_string()).unwrap_or_else(|| "Infinity".to_string());
        let compact_flag = if compact { "true" } else { "false" };

        let expr = format!(
            r#"
            (function() {{
                function describe(el, depth) {{
                    if (depth <= 0) return '';
                    const tag = el.tagName ? el.tagName.toLowerCase() : '#text';
                    const attrs = [];
                    if (el.id) attrs.push('id="' + el.id + '"');
                    if (el.className && typeof el.className === 'string') attrs.push('class="' + el.className + '"');
                    if (el.type) attrs.push('type="' + el.type + '"');
                    if (el.placeholder) attrs.push('placeholder="' + el.placeholder + '"');
                    if (el.href) attrs.push('href="' + el.href.substring(0, 80) + '"');
                    const visible = el.offsetWidth > 0 && el.offsetHeight > 0 && window.getComputedStyle(el).display !== 'none';
                    let text = '';
                    if ({compact} && !visible) return '';
                    if (el.childNodes.length === 0 || (el.childNodes.length === 1 && el.childNodes[0].nodeType === 3)) {{
                        text = (el.innerText || '').trim().substring(0, 200);
                        if (text) text = ' ' + text;
                    }}
                    const attrStr = attrs.length ? ' ' + attrs.join(' ') : '';
                    const children = Array.from(el.children || []);
                    const childStr = children.map(c => describe(c, depth - 1)).filter(Boolean).join('\\n');
                    const indent = '  '.repeat(Math.max(0, 3 - depth));
                    if (childStr) return indent + '<' + tag + attrStr + '>' + text + '\\n' + childStr + '\\n' + indent + '</' + tag + '>';
                    return indent + '<' + tag + attrStr + '>' + text + '</' + tag + '>';
                }}
                return describe(document.body, {depth});
            }})()
            "#,
            compact = compact_flag,
            depth = depth_expr,
        );
        self.evaluate(&expr, false)
            .await
            .map(|v| {
                v.get("result")
                    .and_then(|r| r.get("value"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            })
    }

    // ── Emulation ──────────────────────────────────────────────

    pub async fn emulate_device(&self, name: &str) -> Result<Value> {
        let devices: Vec<(&str, i64, i64, f64)> = vec![
            ("iPhone SE", 375, 667, 2.0),
            ("iPhone 12", 390, 844, 3.0),
            ("iPhone 14 Pro", 393, 852, 3.0),
            ("iPad", 768, 1024, 2.0),
            ("iPad Pro", 1024, 1366, 2.0),
            ("Pixel 5", 393, 851, 2.75),
            ("Galaxy S20", 360, 800, 3.0),
        ];
        let device = devices
            .iter()
            .find(|(n, _, _, _)| n.to_lowercase() == name.to_lowercase());
        match device {
            Some((_, w, h, dsf)) => self.set_viewport(*w, *h, Some(*dsf)).await,
            None => Err(anyhow!("Unknown device: {}. Available: {}", name, devices.iter().map(|(n, _, _, _)| *n).collect::<Vec<_>>().join(", "))),
        }
    }

    // ── Network interception ───────────────────────────────────

    pub async fn enable_network(&self) -> Result<Value> {
        self.send_command("Network.enable", json!({})).await
    }

    pub async fn intercept_request(&self, pattern: &str, _response: Option<Value>) -> Result<Value> {
        self.send_command(
            "Fetch.enable",
            json!({
                "patterns": [{ "urlPattern": pattern, "requestStage": "Request" }],
            }),
        )
        .await
    }

    pub async fn disable_fetch_interception(&self) -> Result<Value> {
        self.send_command("Fetch.disable", json!({})).await
    }
}

fn key_to_vk(key: &str) -> i32 {
    match key {
        "Backspace" | "Delete" => 8,
        "Tab" => 9,
        "Enter" => 13,
        "Escape" => 27,
        " " => 32,
        "ArrowLeft" => 37,
        "ArrowUp" => 38,
        "ArrowRight" => 39,
        "ArrowDown" => 40,
        _ => 0,
    }
}

fn find_browser_binary(custom_path: Option<&str>) -> Result<String> {
    if let Some(p) = custom_path {
        if std::path::Path::new(p).exists() {
            return Ok(p.to_string());
        }
        if which::which(p).is_ok() {
            return Ok(p.to_string());
        }
        return Err(anyhow!("Custom browser path not found: {}", p));
    }

    #[cfg(target_os = "windows")]
    {
        let home = dirs::home_dir().unwrap_or_default();
        let candidates: Vec<&str> = vec![
            // Chrome
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            // Edge
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
            // Brave
            r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
            r"C:\Program Files (x86)\BraveSoftware\Brave-Browser\Application\brave.exe",
            // Opera
            r"C:\Program Files\Opera\opera.exe",
            // Vivaldi
            r"C:\Program Files\Vivaldi\Application\vivaldi.exe",
            r"C:\Program Files (x86)\Vivaldi\Application\vivaldi.exe",
            // Chromium
            r"C:\Program Files\Chromium\Application\chrome.exe",
        ];
        let home_candidates: Vec<std::path::PathBuf> = vec![
            home.join("AppData").join("Local").join("Google").join("Chrome").join("Application").join("chrome.exe"),
            home.join("AppData").join("Local").join("Microsoft").join("Edge").join("Application").join("msedge.exe"),
            home.join("AppData").join("Local").join("BraveSoftware").join("Brave-Browser").join("Application").join("brave.exe"),
            home.join("AppData").join("Local").join("Vivaldi").join("Application").join("vivaldi.exe"),
        ];

        for c in &candidates {
            if std::path::Path::new(c).exists() {
                return Ok(c.to_string());
            }
        }
        for c in &home_candidates {
            if c.exists() {
                return Ok(c.to_string_lossy().to_string());
            }
        }

        Err(anyhow!("No Chromium-based browser found. Install Chrome, Edge, Brave, or set browser.native_chrome_path."))
    }

    #[cfg(target_os = "macos")]
    {
        let candidates: Vec<&str> = vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Vivaldi.app/Contents/MacOS/Vivaldi",
            "/Applications/Opera.app/Contents/MacOS/Opera",
        ];
        for c in &candidates {
            if std::path::Path::new(c).exists() {
                return Ok(c.to_string());
            }
        }

        // Try homebrew casks
        let homebrew_candidates = vec![
            "/opt/homebrew/Caskroom/google-chrome/current/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/usr/local/Caskroom/google-chrome/current/Google Chrome.app/Contents/MacOS/Google Chrome",
        ];
        for c in &homebrew_candidates {
            if std::path::Path::new(c).exists() {
                return Ok(c.to_string());
            }
        }

        Err(anyhow!("No Chromium-based browser found. Install Chrome, Edge, Brave, or set browser.native_chrome_path."))
    }

    #[cfg(target_os = "linux")]
    {
        let candidates = vec![
            "google-chrome-stable",
            "google-chrome",
            "chromium-browser",
            "chromium",
            "microsoft-edge-stable",
            "microsoft-edge",
            "brave-browser-stable",
            "brave-browser",
            "vivaldi-stable",
            "vivaldi",
            "opera-stable",
            "opera",
        ];
        for c in &candidates {
            if which::which(c).is_ok() {
                return Ok((*c).to_string());
            }
        }

        let flatpak_paths = vec![
            "/var/lib/flatpak/exports/bin/com.google.Chrome",
            "/var/lib/flatpak/exports/bin/com.microsoft.Edge",
            "/var/lib/flatpak/exports/bin/com.brave.Browser",
        ];
        for p in &flatpak_paths {
            if std::path::Path::new(p).exists() {
                return Ok(p.to_string());
            }
        }

        let snap_names = vec!["chrome", "chromium", "brave"];
        for name in &snap_names {
            let snap_path = format!("/snap/bin/{}", name);
            if std::path::Path::new(&snap_path).exists() {
                return Ok(snap_path);
            }
        }

        Err(anyhow!("No Chromium-based browser found. Install Chrome, Edge, Brave, or set browser.native_chrome_path."))
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err(anyhow!("Auto-launch not supported on this platform. Set browser.native_chrome_path."))
    }
}

fn default_profile_dir(workspace_dir: &str) -> String {
    let path = std::path::Path::new(workspace_dir)
        .join(".openpaw")
        .join("browser-profile");
    path.to_string_lossy().to_string()
}

pub async fn launch_browser(
    cdp_port: u16,
    headless: bool,
    browser_path: Option<&str>,
    workspace_dir: &str,
    profile_dir_override: Option<&str>,
) -> Result<std::process::Child> {
    let browser_bin = find_browser_binary(browser_path)?;

    let profile_dir = profile_dir_override
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_profile_dir(workspace_dir));

    std::fs::create_dir_all(&profile_dir)
        .context("Failed to create browser profile directory")?;

    // Remove Chrome's SingletonLock so a leftover crashed/killed instance doesn't
    // prevent a fresh launch (Chrome refuses to start a second profile instance).
    let singleton_lock = std::path::Path::new(&profile_dir).join("SingletonLock");
    if singleton_lock.exists() {
        tracing::warn!("Removing stale SingletonLock from browser profile: {}", singleton_lock.display());
        let _ = std::fs::remove_file(&singleton_lock);
    }
    // Also remove the SingletonCookie / SingletonSocket on Linux/macOS
    for artifact in &["SingletonCookie", "SingletonSocket"] {
        let p = std::path::Path::new(&profile_dir).join(artifact);
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }

    let mut cmd = std::process::Command::new(&browser_bin);
    cmd.arg(format!("--remote-debugging-port={}", cdp_port))
        .arg("--remote-debugging-address=127.0.0.1")
        .arg(format!("--user-data-dir={}", profile_dir))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-networking")
        .arg("--disable-client-side-phishing-detection")
        .arg("--disable-default-apps")
        .arg("--disable-extensions")
        .arg("--disable-hang-monitor")
        .arg("--disable-popup-blocking")
        .arg("--disable-prompt-on-repost")
        .arg("--disable-sync")
        .arg("--disable-translate")
        .arg("--metrics-recording-only")
        .arg("--safebrowsing-disable-auto-update")
        .arg("--disable-features=TranslateUI")
        .arg("--disable-component-extensions-with-background-pages");

    if headless {
        cmd.arg("--headless=new")
           // Required for reliable headless operation on Windows
           .arg("--disable-gpu")
           .arg("--disable-software-rasterizer")
           .arg("--disable-dev-shm-usage");
    }

    cmd.arg("about:blank");

    tracing::info!("Launching browser: {} with profile at {}", browser_bin, profile_dir);

    let child = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to launch browser: {}", browser_bin))?;

    // Give Chrome more time to initialize its DevTools endpoint.
    // 3 s is usually sufficient; the caller retries up to 5× with 1 s gaps.
    tokio::time::sleep(tokio::time::Duration::from_millis(3000)).await;

    Ok(child)
}