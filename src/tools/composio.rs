use super::{Tool, ToolContext, ToolResult};
use anyhow::{Context, Result};
use reqwest::Method;
use reqwest::Url;
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::time::Duration;

const COMPOSIO_API_BASE: &str = "https://backend.composio.dev/api/v3";

pub struct ComposioTool {
    pub api_key: String,
    pub entity_id: String,
}

struct ApiResponse {
    status: u16,
    body_text: String,
    body_json: Option<Value>,
}

#[derive(Clone, Debug)]
struct AuthConfigSummary {
    id: String,
    toolkit_slug: Option<String>,
    name: Option<String>,
    status: Option<String>,
    is_composio_managed: Option<bool>,
}

impl Tool for ComposioTool {
    fn name(&self) -> &str {
        "composio"
    }

    fn description(&self) -> &str {
        "Use Composio app integrations. Actions: list tools, execute a tool slug, or connect an account for OAuth apps like Gmail."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"action":{"type":"string","enum":["list","execute","connect"],"description":"Operation to perform"},"app":{"type":"string","description":"Toolkit/app name (e.g. 'gmail', 'github'). Alias of toolkit_slug."},"toolkit_slug":{"type":"string","description":"Composio toolkit slug (e.g. 'gmail')."},"search":{"type":"string","description":"Optional text filter when listing tools."},"query":{"type":"string","description":"Alias of search."},"tool_slug":{"type":"string","description":"Composio tool slug to execute (recommended)."},"action_name":{"type":"string","description":"Legacy alias of tool_slug."},"params":{"type":"object","description":"Arguments passed to the Composio tool execute call."},"entity_id":{"type":"string","description":"Optional user/entity override. Defaults to config.composio.entity_id."},"connected_account_id":{"type":"string","description":"Optional connected account id for execute."},"auth_config_id":{"type":"string","description":"Auth config id for connect flow (auto-discovered if app/toolkit_slug is provided)."},"callback_url":{"type":"string","description":"Optional OAuth callback URL for connect link session."}},"required":["action"]}"#.to_string()
    }

    fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return Ok(ToolResult::fail("Missing 'action' parameter")),
        };

        match action {
            "list" => self.list_actions(&args),
            "execute" => self.execute_action(&args),
            "connect" => self.connect_account(&args),
            _ => Ok(ToolResult::fail(format!("Unknown action: {}", action))),
        }
    }
}

impl ComposioTool {
    fn client(&self) -> Result<Client> {
        Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .context("Failed to build HTTP client")
    }

    fn api_request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<ApiResponse> {
        let mut url = Url::parse(&format!("{}{}", COMPOSIO_API_BASE, path))
            .with_context(|| format!("Invalid Composio URL path: {}", path))?;
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in query {
                pairs.append_pair(k, v);
            }
        }
        let client = self.client()?;

        let url_for_error = url.to_string();
        let mut req = client
            .request(method, url)
            .header("x-api-key", &self.api_key)
            .header("accept", "application/json");

        if let Some(payload) = body {
            req = req.json(payload);
        }

        let resp = req
            .send()
            .with_context(|| format!("Composio request failed: {}", url_for_error))?;
        let status = resp.status().as_u16();
        let body_text = resp
            .text()
            .unwrap_or_else(|_| "(failed to read response body)".to_string());
        let body_json = serde_json::from_str::<Value>(&body_text).ok();

        Ok(ApiResponse {
            status,
            body_text,
            body_json,
        })
    }

    fn list_actions(&self, args: &Value) -> Result<ToolResult> {
        let toolkit_slug = toolkit_from_args(args);
        let search_term = args
            .get("search")
            .or(args.get("query"))
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string());

        let mut query = vec![("limit", "100".to_string())];
        if let Some(slug) = &toolkit_slug {
            query.push(("toolkit_slug", slug.clone()));
        }
        if let Some(search) = &search_term {
            if !search.is_empty() {
                query.push(("query", search.clone()));
            }
        }

        let mut resp = self.api_request(Method::GET, "/tools", &query, None)?;
        if !is_success(resp.status) {
            return Ok(ToolResult::fail(format!(
                "Composio list failed (HTTP {}): {}",
                resp.status,
                one_line(&resp.body_text)
            )));
        }

        // Fallback: if toolkit filter returns zero items, retry with search only.
        if let (Some(slug), Some(json_body)) = (toolkit_slug.as_deref(), resp.body_json.as_ref()) {
            if extract_array_entries(json_body)
                .map(|a| a.is_empty())
                .unwrap_or(false)
            {
                let fallback_query =
                    vec![("limit", "100".to_string()), ("query", slug.to_string())];
                let fallback_resp =
                    self.api_request(Method::GET, "/tools", &fallback_query, None)?;
                if is_success(fallback_resp.status) {
                    resp = fallback_resp;
                }
            }
        }

        let Some(json_body) = resp.body_json else {
            return Ok(ToolResult::ok(resp.body_text));
        };

        Ok(ToolResult::ok(render_list_output(
            &json_body,
            toolkit_slug.as_deref(),
        )))
    }

    fn execute_action(&self, args: &Value) -> Result<ToolResult> {
        let action = match args
            .get("tool_slug")
            .or(args.get("action_name"))
            .and_then(|v| v.as_str())
        {
            Some(a) if !a.trim().is_empty() => a.trim(),
            _ => return Ok(ToolResult::fail("Missing 'tool_slug' or 'action_name'")),
        };

        let user_id = args
            .get("entity_id")
            .or(args.get("user_id"))
            .and_then(|v| v.as_str())
            .unwrap_or(&self.entity_id);

        let params = args
            .get("params")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));

        let mut payload = json!({
            "arguments": params,
            "user_id": user_id
        });

        if let Some(connected_account_id) =
            args.get("connected_account_id").and_then(|v| v.as_str())
        {
            payload["connected_account_id"] = json!(connected_account_id);
        }

        let primary_path = format!("/tools/execute/{}", action);
        let mut resp = self.api_request(Method::POST, &primary_path, &[], Some(&payload))?;

        // Fallback for common case where LLM passes lowercase slug.
        let fallback_slug = action.to_ascii_uppercase();
        if resp.status == 404 && fallback_slug != action {
            let fallback_path = format!("/tools/execute/{}", fallback_slug);
            resp = self.api_request(Method::POST, &fallback_path, &[], Some(&payload))?;
        }

        if !is_success(resp.status) {
            return Ok(ToolResult::fail(format!(
                "Composio execute failed (HTTP {}): {}",
                resp.status,
                one_line(&resp.body_text)
            )));
        }

        Ok(ToolResult::ok(pretty_json_or_text(
            resp.body_json,
            resp.body_text,
        )))
    }

    fn connect_account(&self, args: &Value) -> Result<ToolResult> {
        let user_id = args
            .get("entity_id")
            .or(args.get("user_id"))
            .and_then(|v| v.as_str())
            .unwrap_or(&self.entity_id);
        let toolkit_slug = toolkit_from_args(args);
        let explicit_auth_config_id = args
            .get("auth_config_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(|id| id.to_string());

        if let Some(auth_config_id) = explicit_auth_config_id {
            if self.get_auth_config(&auth_config_id)?.is_none() {
                let candidates = self.discover_auth_configs(toolkit_slug.as_deref())?;
                return Ok(ToolResult::fail(format!(
                    "Composio auth_config_id '{}' was not found for this API key/project.{}",
                    auth_config_id,
                    render_auth_config_candidates(&candidates)
                )));
            }
            return self.create_link_session(
                &auth_config_id,
                user_id,
                args,
                toolkit_slug.as_deref(),
            );
        }

        let Some(toolkit_slug) = toolkit_slug else {
            return Ok(ToolResult::fail(
                "Connect requires either 'auth_config_id' or app/toolkit_slug (for example app='gmail').",
            ));
        };

        let candidates = self.discover_auth_configs(Some(&toolkit_slug))?;
        let Some(best) = choose_best_auth_config(&candidates) else {
            return Ok(ToolResult::fail(format!(
                "No auth configs found for toolkit '{}'. Create one in Composio dashboard or pass auth_config_id explicitly.",
                toolkit_slug
            )));
        };

        self.create_link_session(&best.id, user_id, args, Some(&toolkit_slug))
    }

    fn create_link_session(
        &self,
        auth_config_id: &str,
        user_id: &str,
        args: &Value,
        toolkit_slug: Option<&str>,
    ) -> Result<ToolResult> {
        let mut body = json!({
            "auth_config_id": auth_config_id,
            "user_id": user_id
        });
        if let Some(callback_url) = args.get("callback_url").and_then(|v| v.as_str()) {
            body["callback_url"] = json!(callback_url);
        }

        let resp = self.api_request(Method::POST, "/connected_accounts/link", &[], Some(&body))?;
        if !is_success(resp.status) {
            let base_error = format!(
                "Composio connect failed (HTTP {}): {}",
                resp.status,
                composio_error_message(resp.body_json.as_ref(), &resp.body_text)
            );

            if matches!(resp.status, 400 | 404 | 422) {
                let candidates = self.discover_auth_configs(toolkit_slug)?;
                return Ok(ToolResult::fail(format!(
                    "{}{}",
                    base_error,
                    render_auth_config_candidates(&candidates)
                )));
            }

            return Ok(ToolResult::fail(base_error));
        }

        if let Some(ref v) = resp.body_json {
            if let Some(url) = extract_auth_url(v) {
                return Ok(ToolResult::ok(format!(
                    "Open this URL to authenticate {}: {}\n\n{}",
                    user_id,
                    url,
                    pretty_json_or_text(resp.body_json, resp.body_text)
                )));
            }
        }

        Ok(ToolResult::ok(pretty_json_or_text(
            resp.body_json,
            resp.body_text,
        )))
    }

    fn discover_auth_configs(&self, toolkit_slug: Option<&str>) -> Result<Vec<AuthConfigSummary>> {
        let mut query = vec![("limit", "100".to_string())];
        if let Some(slug) = toolkit_slug {
            query.push(("toolkit_slug", slug.to_string()));
            query.push(("query", slug.to_string()));
        }
        let resp = self.api_request(Method::GET, "/auth_configs", &query, None)?;
        if !is_success(resp.status) {
            return Ok(Vec::new());
        }

        let Some(json_body) = resp.body_json else {
            return Ok(Vec::new());
        };

        let Some(items) = extract_array_entries(&json_body) else {
            return Ok(Vec::new());
        };

        let mut configs: Vec<AuthConfigSummary> =
            items.iter().filter_map(parse_auth_config).collect();
        if let Some(slug) = toolkit_slug {
            configs.retain(|c| c.toolkit_slug.as_deref() == Some(slug));
        }
        configs.sort_by_key(auth_config_sort_key);
        Ok(configs)
    }

    fn get_auth_config(&self, auth_config_id: &str) -> Result<Option<AuthConfigSummary>> {
        let path = format!("/auth_configs/{}", auth_config_id);
        let resp = self.api_request(Method::GET, &path, &[], None)?;
        if resp.status == 404 {
            return Ok(None);
        }
        if !is_success(resp.status) {
            return Ok(None);
        }
        Ok(resp.body_json.as_ref().and_then(parse_auth_config))
    }
}

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

fn toolkit_from_args(args: &Value) -> Option<String> {
    args.get("toolkit_slug")
        .or(args.get("app"))
        .or(args.get("toolkit"))
        .and_then(Value::as_str)
        .map(normalize_toolkit_slug)
}

fn normalize_toolkit_slug(raw: &str) -> String {
    raw.trim().replace([' ', '_'], "-").to_ascii_lowercase()
}

fn extract_array_entries(value: &Value) -> Option<&[Value]> {
    if let Some(arr) = value.as_array() {
        return Some(arr.as_slice());
    }

    for key in ["items", "data", "results", "tools"] {
        if let Some(arr) = value.get(key).and_then(Value::as_array) {
            return Some(arr.as_slice());
        }
    }

    if let Some(data) = value.get("data") {
        for key in ["items", "results", "tools"] {
            if let Some(arr) = data.get(key).and_then(Value::as_array) {
                return Some(arr.as_slice());
            }
        }
    }

    None
}

fn parse_auth_config(value: &Value) -> Option<AuthConfigSummary> {
    let id = value.get("id").and_then(Value::as_str)?.to_string();
    let toolkit_slug = value
        .get("toolkit_slug")
        .and_then(Value::as_str)
        .map(normalize_toolkit_slug)
        .or_else(|| {
            value
                .get("toolkit")
                .and_then(Value::as_object)
                .and_then(|t| t.get("slug"))
                .and_then(Value::as_str)
                .map(normalize_toolkit_slug)
        })
        .or_else(|| {
            value
                .get("toolkit")
                .and_then(Value::as_str)
                .map(normalize_toolkit_slug)
        });

    let name = value
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_string);
    let is_composio_managed = value.get("is_composio_managed").and_then(Value::as_bool);

    Some(AuthConfigSummary {
        id,
        toolkit_slug,
        name,
        status,
        is_composio_managed,
    })
}

fn choose_best_auth_config(configs: &[AuthConfigSummary]) -> Option<&AuthConfigSummary> {
    configs.first()
}

fn auth_config_sort_key(cfg: &AuthConfigSummary) -> (u8, u8, String) {
    let status_rank = match cfg.status.as_deref() {
        Some("active") | Some("ACTIVE") | Some("enabled") | Some("ENABLED") => 0,
        _ => 1,
    };
    let managed_rank = if cfg.is_composio_managed == Some(true) {
        0
    } else {
        1
    };
    (status_rank, managed_rank, cfg.id.clone())
}

fn render_auth_config_candidates(configs: &[AuthConfigSummary]) -> String {
    if configs.is_empty() {
        return String::new();
    }

    let mut lines = vec!["\nAvailable auth_config_id candidates:".to_string()];
    for cfg in configs.iter().take(10) {
        let toolkit = cfg.toolkit_slug.as_deref().unwrap_or("unknown-toolkit");
        let status = cfg.status.as_deref().unwrap_or("unknown-status");
        let name = cfg.name.as_deref().unwrap_or("unnamed");
        lines.push(format!(
            "\n- {} ({}, {}, {})",
            cfg.id, toolkit, status, name
        ));
    }
    if configs.len() > 10 {
        lines.push(format!(
            "\n... and {} more auth configs.",
            configs.len() - 10
        ));
    }
    lines.join("")
}

fn extract_auth_url(value: &Value) -> Option<String> {
    for key in ["redirect_url", "auth_url", "url", "link"] {
        if let Some(url) = value.get(key).and_then(Value::as_str) {
            return Some(url.to_string());
        }
    }

    if let Some(link_obj) = value.get("link") {
        if let Some(url) = link_obj.get("url").and_then(Value::as_str) {
            return Some(url.to_string());
        }
    }

    if let Some(data) = value.get("data") {
        return extract_auth_url(data);
    }

    None
}

fn composio_error_message(body_json: Option<&Value>, body_text: &str) -> String {
    if let Some(v) = body_json {
        if let Some(msg) = v.get("message").and_then(Value::as_str) {
            return msg.to_string();
        }
        if let Some(msg) = v
            .get("error")
            .and_then(Value::as_object)
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
        {
            return msg.to_string();
        }
    }
    one_line(body_text)
}

fn render_list_output(value: &Value, toolkit_filter: Option<&str>) -> String {
    let Some(items) = extract_array_entries(value) else {
        return pretty_json_or_text(Some(value.clone()), String::new());
    };

    let mut lines = Vec::new();
    match toolkit_filter {
        Some(filter) => lines.push(format!(
            "Found {} Composio tools for toolkit {}:",
            items.len(),
            filter
        )),
        None => lines.push(format!("Found {} Composio tools:", items.len())),
    }

    for item in items.iter().take(80) {
        let slug = item
            .get("slug")
            .or(item.get("tool_slug"))
            .and_then(Value::as_str)
            .unwrap_or("unknown_tool_slug");

        let toolkit = item
            .get("toolkit_slug")
            .and_then(Value::as_str)
            .or_else(|| item.get("toolkit").and_then(Value::as_str))
            .or_else(|| {
                item.get("toolkit")
                    .and_then(|v| v.get("slug"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("unknown_toolkit");

        let display_name = item.get("name").and_then(Value::as_str).unwrap_or(slug);

        lines.push(format!("- {} ({}) [{}]", slug, toolkit, display_name));
    }

    if items.len() > 80 {
        lines.push(format!(
            "... truncated {} additional tool(s).",
            items.len() - 80
        ));
    }

    lines.push("Use action='execute' with 'tool_slug' exactly as listed above.".to_string());
    lines.push(
        "If you need a specific app (e.g. Gmail), call list with app='gmail' or search='gmail'."
            .to_string(),
    );
    lines.join("\n")
}

fn pretty_json_or_text(json_body: Option<Value>, body_text: String) -> String {
    if let Some(v) = json_body {
        serde_json::to_string_pretty(&v).unwrap_or_else(|_| body_text)
    } else {
        body_text
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_toolkit_slug_handles_spaces_and_hyphens() {
        assert_eq!(normalize_toolkit_slug("gmail"), "gmail");
        assert_eq!(normalize_toolkit_slug("google-calendar"), "google-calendar");
        assert_eq!(normalize_toolkit_slug("  github app  "), "github-app");
    }

    #[test]
    fn extract_array_entries_supports_common_shapes() {
        let root = json!([{"id":"a"}]);
        assert_eq!(extract_array_entries(&root).map(|a| a.len()), Some(1));

        let items = json!({"items":[{"id":"a"},{"id":"b"}]});
        assert_eq!(extract_array_entries(&items).map(|a| a.len()), Some(2));

        let nested = json!({"data":{"tools":[{"id":"a"}]}});
        assert_eq!(extract_array_entries(&nested).map(|a| a.len()), Some(1));
    }
}
