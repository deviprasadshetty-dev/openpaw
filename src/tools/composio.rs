use super::{Tool, ToolContext, ToolResult};
use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::{Client, Method, Url};
use serde_json::{Value, json};
use std::time::Duration;

const COMPOSIO_API_BASE: &str = "https://backend.composio.dev/api/v3";
const COMPOSIO_API_BASE_V31: &str = "https://backend.composio.dev/api/v3.1";

pub struct ComposioTool {
    pub api_key: String,
    pub user_id: String,
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

#[derive(Clone, Debug)]
struct ConnectedAccountSummary {
    id: String,
    toolkit_slug: Option<String>,
    status: Option<String>,
}

#[async_trait]
impl Tool for ComposioTool {
    fn name(&self) -> &str {
        "composio"
    }

    fn description(&self) -> &str {
        "Integrated app platform (Gmail, Calendar, GitHub, Slack, etc.). PRIMARY WORKFLOW: Use action='query' with 'text' (natural language) for ANY request (e.g., 'check my emails', 'add a meeting for 2pm tomorrow'). If you get an authentication error, use action='connect' with app='app_name' to provide the user with a login link. For event monitoring, use the trigger actions to discover trigger types, create triggers, list them, and enable/disable/delete them."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"action":{"type":"string","enum":["query","execute","connect","list","tool_router","trigger_types","trigger_type","trigger_create","trigger_list","trigger_enable","trigger_disable","trigger_delete"],"description":"Operation to perform. 'query' (RECOMMENDED) handles discovery and execution via natural language; 'connect' handles app authorization; 'execute' runs a specific tool; 'list' discovers tools manually; 'tool_router' creates an isolated execution session. Trigger actions manage event-based Composio triggers."},"text":{"type":"string","description":"The natural language request."},"app":{"type":"string","description":"App name / toolkit slug for 'connect', 'list', or trigger discovery, for example 'gmail' or 'github'."},"tool_slug":{"type":"string","description":"Exact tool ID for manual 'execute'."},"trigger_slug":{"type":"string","description":"Trigger type slug for trigger actions, for example 'GMAIL_NEW_GMAIL_MESSAGE' or 'GITHUB_COMMIT_EVENT'."},"trigger_id":{"type":"string","description":"Active trigger instance ID for enable/disable/delete."},"params":{"type":"object","description":"Structured arguments for manual 'execute'."},"trigger_config":{"type":"object","description":"Configuration object required by a trigger type when creating a trigger."},"toolkit_versions":{"description":"Optional toolkit version pin for trigger creation. Either a string like 'latest' or an object mapping toolkit slugs to versions."},"user_id":{"type":"string","description":"Optional Composio user_id override."},"entity_id":{"type":"string","description":"Legacy alias for user_id."},"connected_account_id":{"type":"string","description":"Optional connected account ID. Recommended when the user has multiple accounts for the same toolkit."},"callback_url":{"type":"string","description":"Optional OAuth callback URL."},"session_id":{"type":"string","description":"Session ID for isolated tool router execution."},"toolkits":{"type":"array","items":{"type":"string"},"description":"List of toolkit slugs for tool_router."},"tools":{"type":"array","items":{"type":"string"},"description":"List of specific tool slugs for tool_router."},"tags":{"type":"array","items":{"type":"string"},"description":"List of tags for tool_router."},"show_disabled":{"type":"boolean","description":"For trigger_list: include disabled triggers."},"limit":{"type":"integer","description":"Optional result limit for listing trigger types or active triggers."},"cursor":{"type":"string","description":"Pagination cursor for trigger_list."}},"required":["action"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return Ok(ToolResult::fail("Missing 'action' parameter")),
        };

        match action {
            "list" => self.list_actions(&args).await,
            "execute" => self.execute_action(&args).await,
            "connect" => self.connect_account(&args).await,
            "tool_router" => self.tool_router_action(&args).await,
            "query" => self.query_action(&args).await,
            "trigger_types" => self.list_trigger_types_action(&args).await,
            "trigger_type" => self.get_trigger_type_action(&args).await,
            "trigger_create" => self.create_trigger_action(&args).await,
            "trigger_list" => self.list_triggers_action(&args).await,
            "trigger_enable" => self.set_trigger_status_action(&args, "enable").await,
            "trigger_disable" => self.set_trigger_status_action(&args, "disable").await,
            "trigger_delete" => self.delete_trigger_action(&args).await,
            _ => Ok(ToolResult::fail(format!("Unknown action: {}", action))),
        }
    }
}

impl ComposioTool {
    fn client(&self) -> Result<Client> {
        Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to build HTTP client")
    }

    async fn api_request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<ApiResponse> {
        self.api_request_base(COMPOSIO_API_BASE, method, path, query, body)
            .await
    }

    async fn api_request_v31(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<ApiResponse> {
        self.api_request_base(COMPOSIO_API_BASE_V31, method, path, query, body)
            .await
    }

    async fn api_request_base(
        &self,
        base_url: &str,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<ApiResponse> {
        let mut url = Url::parse(&format!("{}{}", base_url, path))
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
            .await
            .with_context(|| format!("Composio request failed: {}", url_for_error))?;
        let status = resp.status().as_u16();
        let body_text = resp
            .text()
            .await
            .unwrap_or_else(|_| "(failed to read response body)".to_string());
        let body_json = serde_json::from_str::<Value>(&body_text).ok();

        Ok(ApiResponse {
            status,
            body_text,
            body_json,
        })
    }

    async fn list_actions(&self, args: &Value) -> Result<ToolResult> {
        let toolkit_slug = toolkit_from_args(args);
        let search_term = args
            .get("search")
            .or(args.get("query"))
            .or(args.get("text"))
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string());

        let mut query = vec![("limit", "100".to_string())];
        if let Some(slug) = &toolkit_slug {
            query.push(("toolkit_slug", slug.clone()));
        }
        if let Some(search) = &search_term
            && !search.is_empty()
        {
            query.push(("query", search.clone()));
        }

        let mut resp = self
            .api_request(Method::GET, "/tools", &query, None)
            .await?;
        if !is_success(resp.status) {
            return Ok(ToolResult::fail(format!(
                "Composio list failed (HTTP {}): {}",
                resp.status,
                one_line(&resp.body_text)
            )));
        }

        if let (Some(slug), Some(json_body)) = (toolkit_slug.as_deref(), resp.body_json.as_ref())
            && extract_array_entries(json_body)
                .map(|a| a.is_empty())
                .unwrap_or(false)
        {
            let fallback_query = vec![("limit", "100".to_string()), ("query", slug.to_string())];
            let fallback_resp = self
                .api_request(Method::GET, "/tools", &fallback_query, None)
                .await?;
            if is_success(fallback_resp.status) {
                resp = fallback_resp;
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

    async fn execute_action(&self, args: &Value) -> Result<ToolResult> {
        let tool_slug = match args
            .get("tool_slug")
            .or(args.get("action_name"))
            .and_then(|v| v.as_str())
        {
            Some(a) if !a.trim().is_empty() => a.trim(),
            _ => return Ok(ToolResult::fail("Missing 'tool_slug' or 'action_name'")),
        };

        let user_id = args
            .get("user_id")
            .or(args.get("entity_id"))
            .and_then(|v| v.as_str())
            .unwrap_or(&self.user_id);

        if let Some(session_id) = args.get("session_id").and_then(|v| v.as_str()) {
            return self
                .execute_tool_router_session(session_id, tool_slug, args)
                .await;
        }

        let mut payload = json!({});

        payload["user_id"] = json!(user_id);
        if let Some(connected_account_id) =
            args.get("connected_account_id").and_then(|v| v.as_str())
        {
            payload["connected_account_id"] = json!(connected_account_id);
        }

        let params = args.get("params");
        let text = args.get("text").and_then(Value::as_str);

        if let Some(p) = params {
            if !p.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                payload["arguments"] = p.clone();
            } else if let Some(t) = text {
                payload["text"] = json!(t);
            }
        } else if let Some(t) = text {
            payload["text"] = json!(t);
        }

        let primary_path = format!("/tools/execute/{}", tool_slug);
        let mut resp = self
            .api_request(Method::POST, &primary_path, &[], Some(&payload))
            .await?;

        let fallback_slug = tool_slug.to_ascii_uppercase();
        if resp.status == 404 && fallback_slug != tool_slug {
            let fallback_path = format!("/tools/execute/{}", fallback_slug);
            resp = self
                .api_request(Method::POST, &fallback_path, &[], Some(&payload))
                .await?;
        }

        if !is_success(resp.status) {
            return Ok(ToolResult::fail(format!(
                "Composio execute failed (HTTP {}): {}",
                resp.status,
                composio_error_message(resp.body_json.as_ref(), &resp.body_text)
            )));
        }

        Ok(ToolResult::ok(pretty_json_or_text(
            resp.body_json,
            resp.body_text,
        )))
    }

    async fn execute_tool_router_session(
        &self,
        session_id: &str,
        tool_slug: &str,
        args: &Value,
    ) -> Result<ToolResult> {
        let mut payload = json!({
            "tool_slug": tool_slug,
        });

        if let Some(params) = args.get("params")
            && !params.as_object().map(|o| o.is_empty()).unwrap_or(true)
        {
            payload["arguments"] = params.clone();
        } else if let Some(text) = args.get("text").and_then(Value::as_str) {
            payload["arguments"] = json!({ "text": text });
        }

        if let Some(account) = args
            .get("account")
            .or(args.get("connected_account_id"))
            .and_then(Value::as_str)
        {
            payload["account"] = json!(account);
        }

        let path = format!("/tool_router/session/{}/execute", session_id);
        let mut resp = self
            .api_request_v31(Method::POST, &path, &[], Some(&payload))
            .await?;

        let fallback_slug = tool_slug.to_ascii_uppercase();
        if resp.status == 404 && fallback_slug != tool_slug {
            payload["tool_slug"] = json!(fallback_slug);
            resp = self
                .api_request_v31(Method::POST, &path, &[], Some(&payload))
                .await?;
        }

        if !is_success(resp.status) {
            return Ok(ToolResult::fail(format!(
                "Composio Tool Router execute failed (HTTP {}): {}",
                resp.status,
                composio_error_message(resp.body_json.as_ref(), &resp.body_text)
            )));
        }

        Ok(ToolResult::ok(pretty_json_or_text(
            resp.body_json,
            resp.body_text,
        )))
    }

    async fn query_action(&self, args: &Value) -> Result<ToolResult> {
        let text = match args
            .get("text")
            .or(args.get("query"))
            .and_then(Value::as_str)
        {
            Some(t) => t,
            None => return Ok(ToolResult::fail("Missing 'text' or 'query' parameter")),
        };
        let user_id = args
            .get("user_id")
            .or(args.get("entity_id"))
            .and_then(|v| v.as_str())
            .unwrap_or(&self.user_id);

        let query_params = vec![("query", text.to_string()), ("limit", "5".to_string())];
        let list_resp = self
            .api_request(Method::GET, "/tools", &query_params, None)
            .await?;

        if !is_success(list_resp.status) {
            return Ok(ToolResult::fail(format!(
                "Failed to discover tools for query (HTTP {}): {}",
                list_resp.status, list_resp.body_text
            )));
        }

        let Some(json_body) = list_resp.body_json else {
            return Ok(ToolResult::fail("No tools found for your query."));
        };

        let Some(items) = extract_array_entries(&json_body) else {
            return Ok(ToolResult::fail("No tools found for your query."));
        };

        if items.is_empty() {
            return Ok(ToolResult::fail(format!(
                "Could not find a relevant tool for: '{}'",
                text
            )));
        }

        let best_tool = &items[0];
        let tool_slug = best_tool
            .get("slug")
            .or(best_tool.get("tool_slug"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        let toolkit_slug = best_tool
            .get("toolkit_slug")
            .and_then(Value::as_str)
            .or_else(|| best_tool.get("toolkit").and_then(Value::as_str))
            .or_else(|| {
                best_tool
                    .get("toolkit")
                    .and_then(|v| v.get("slug"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("unknown_toolkit");

        let mut exec_args = args.clone();
        exec_args["tool_slug"] = json!(tool_slug);
        exec_args["action"] = json!("execute");
        exec_args["text"] = json!(text);

        let result = self.execute_action(&exec_args).await?;

        if !result.is_error {
            Ok(ToolResult::ok(format!(
                "Successfully executed natural language query via tool '{}' (toolkit: {}):\n\n{}",
                tool_slug, toolkit_slug, result.content
            )))
        } else {
            let out = &result.content;
            if out.to_lowercase().contains("auth")
                || out.contains("HTTP 401")
                || out.contains("HTTP 422")
            {
                let accounts = self
                    .discover_connected_accounts(Some(&toolkit_slug), user_id)
                    .await
                    .unwrap_or_default();
                if accounts.is_empty() {
                    Ok(ToolResult::fail(format!(
                        "Failed to execute query using tool '{}' (toolkit: {}). This is likely an authentication issue.\n\nPlease use action='connect' with app='{}' to authenticate, then try your query again.\n\nDetails:\n{}",
                        tool_slug, toolkit_slug, toolkit_slug, out
                    )))
                } else {
                    Ok(ToolResult::fail(format!(
                        "Failed to execute query using tool '{}' (toolkit: {}). A connected account already exists, so this does not look like a missing-login problem.\n\nConnected account(s):{}\n\nPlease retry with one of those accounts or inspect the tool-specific error below.\n\nDetails:\n{}",
                        tool_slug,
                        toolkit_slug,
                        render_connected_account_candidates(&accounts),
                        out
                    )))
                }
            } else {
                Ok(ToolResult::fail(format!(
                    "Failed to execute query using tool '{}' (toolkit: {}):\n\n{}",
                    tool_slug, toolkit_slug, out
                )))
            }
        }
    }

    async fn list_trigger_types_action(&self, args: &Value) -> Result<ToolResult> {
        let toolkit_slug = toolkit_from_args(args);
        let mut query = vec![(
            "limit",
            args.get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .to_string(),
        )];
        if let Some(slug) = &toolkit_slug {
            query.push(("toolkit_slug", slug.clone()));
            query.push(("query", slug.clone()));
        }
        if let Some(search) = args
            .get("search")
            .or(args.get("query"))
            .or(args.get("text"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            query.push(("query", search.to_string()));
        }

        let resp = self
            .api_request_v31(Method::GET, "/triggers_types", &query, None)
            .await?;
        if !is_success(resp.status) {
            return Ok(ToolResult::fail(format!(
                "Composio trigger type list failed (HTTP {}): {}",
                resp.status,
                composio_error_message(resp.body_json.as_ref(), &resp.body_text)
            )));
        }

        Ok(ToolResult::ok(render_trigger_types_output(
            resp.body_json.as_ref(),
            &resp.body_text,
            toolkit_slug.as_deref(),
        )))
    }

    async fn get_trigger_type_action(&self, args: &Value) -> Result<ToolResult> {
        let trigger_slug = match args
            .get("trigger_slug")
            .or(args.get("slug"))
            .or(args.get("trigger_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(slug) => slug,
            None => return Ok(ToolResult::fail("Missing 'trigger_slug' for trigger_type")),
        };

        let path = format!("/triggers_types/{}", trigger_slug);
        let resp = self.api_request_v31(Method::GET, &path, &[], None).await?;
        if !is_success(resp.status) {
            return Ok(ToolResult::fail(format!(
                "Composio trigger type lookup failed (HTTP {}): {}",
                resp.status,
                composio_error_message(resp.body_json.as_ref(), &resp.body_text)
            )));
        }

        Ok(ToolResult::ok(pretty_json_or_text(
            resp.body_json,
            resp.body_text,
        )))
    }

    async fn create_trigger_action(&self, args: &Value) -> Result<ToolResult> {
        let trigger_slug = match args
            .get("trigger_slug")
            .or(args.get("slug"))
            .or(args.get("trigger_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(slug) => slug,
            None => return Ok(ToolResult::fail("Missing 'trigger_slug' for trigger_create")),
        };

        let user_id = args
            .get("user_id")
            .or(args.get("entity_id"))
            .and_then(Value::as_str)
            .unwrap_or(&self.user_id);

        let mut connected_account_id = args
            .get("connected_account_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let toolkit_slug = toolkit_from_args(args);

        if connected_account_id.is_none()
            && let Some(slug) = toolkit_slug.as_deref()
        {
            let accounts = self
                .discover_connected_accounts(Some(slug), user_id)
                .await
                .unwrap_or_default();
            if accounts.len() == 1 {
                connected_account_id = Some(accounts[0].id.clone());
            } else if accounts.len() > 1 {
                return Ok(ToolResult::fail(format!(
                    "Multiple connected accounts exist for toolkit '{}'. Please provide 'connected_account_id'. Available account(s):{}",
                    slug,
                    render_connected_account_candidates(&accounts)
                )));
            }
        }

        let connected_account_id = match connected_account_id {
            Some(id) => id,
            None => {
                return Ok(ToolResult::fail(
                    "trigger_create needs 'connected_account_id', or an app/toolkit_slug with exactly one connected account.",
                ))
            }
        };

        let mut payload = json!({
            "connected_account_id": connected_account_id,
        });
        if let Some(cfg) = args.get("trigger_config").or(args.get("params")) {
            if cfg.is_object() {
                payload["trigger_config"] = cfg.clone();
            }
        }
        if let Some(versions) = args.get("toolkit_versions") {
            payload["toolkit_versions"] = versions.clone();
        }

        let path = format!("/trigger_instances/{}/upsert", trigger_slug);
        let resp = self
            .api_request_v31(Method::POST, &path, &[], Some(&payload))
            .await?;
        if !is_success(resp.status) {
            return Ok(ToolResult::fail(format!(
                "Composio trigger create failed (HTTP {}): {}",
                resp.status,
                composio_error_message(resp.body_json.as_ref(), &resp.body_text)
            )));
        }

        let trigger_id = resp
            .body_json
            .as_ref()
            .and_then(|v| v.get("trigger_id").or_else(|| v.get("triggerId")))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        Ok(ToolResult::ok(format!(
            "Created or updated trigger '{}' with trigger_id '{}'.\n\n{}",
            trigger_slug,
            trigger_id,
            pretty_json_or_text(resp.body_json, resp.body_text)
        )))
    }

    async fn list_triggers_action(&self, args: &Value) -> Result<ToolResult> {
        let user_id = args
            .get("user_id")
            .or(args.get("entity_id"))
            .and_then(Value::as_str)
            .unwrap_or(&self.user_id);
        let toolkit_slug = toolkit_from_args(args);
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(100)
            .to_string();
        let show_disabled = args
            .get("show_disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            .to_string();

        let mut query = vec![("limit", limit), ("show_disabled", show_disabled)];
        if let Some(cursor) = args.get("cursor").and_then(Value::as_str) {
            query.push(("cursor", cursor.to_string()));
        }
        if let Some(trigger_id) = args.get("trigger_id").and_then(Value::as_str) {
            query.push(("trigger_ids", format!("[\"{}\"]", trigger_id)));
        }
        if let Some(trigger_slug) = args
            .get("trigger_slug")
            .or(args.get("trigger_name"))
            .and_then(Value::as_str)
        {
            query.push(("trigger_names", format!("[\"{}\"]", trigger_slug)));
        }
        if let Some(account_id) = args.get("connected_account_id").and_then(Value::as_str) {
            query.push(("connected_account_ids", format!("[\"{}\"]", account_id)));
        } else if let Some(slug) = toolkit_slug.as_deref() {
            let accounts = self
                .discover_connected_accounts(Some(slug), user_id)
                .await
                .unwrap_or_default();
            if accounts.len() == 1 {
                query.push((
                    "connected_account_ids",
                    format!("[\"{}\"]", accounts[0].id),
                ));
            }
        }

        let resp = self
            .api_request_v31(Method::GET, "/trigger_instances/active", &query, None)
            .await?;
        if !is_success(resp.status) {
            return Ok(ToolResult::fail(format!(
                "Composio trigger list failed (HTTP {}): {}",
                resp.status,
                composio_error_message(resp.body_json.as_ref(), &resp.body_text)
            )));
        }

        Ok(ToolResult::ok(render_trigger_instances_output(
            resp.body_json.as_ref(),
            &resp.body_text,
        )))
    }

    async fn set_trigger_status_action(&self, args: &Value, status: &str) -> Result<ToolResult> {
        let trigger_id = match args
            .get("trigger_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(id) => id,
            None => {
                return Ok(ToolResult::fail(format!(
                    "Missing 'trigger_id' for trigger_{}",
                    status
                )))
            }
        };

        let path = format!("/trigger_instances/manage/{}", trigger_id);
        let payload = json!({ "status": status });
        let resp = self
            .api_request_v31(Method::PATCH, &path, &[], Some(&payload))
            .await?;
        if !is_success(resp.status) {
            return Ok(ToolResult::fail(format!(
                "Composio trigger {} failed (HTTP {}): {}",
                status,
                resp.status,
                composio_error_message(resp.body_json.as_ref(), &resp.body_text)
            )));
        }

        Ok(ToolResult::ok(format!(
            "Trigger '{}' set to '{}'.\n\n{}",
            trigger_id,
            status,
            pretty_json_or_text(resp.body_json, resp.body_text)
        )))
    }

    async fn delete_trigger_action(&self, args: &Value) -> Result<ToolResult> {
        let trigger_id = match args
            .get("trigger_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(id) => id,
            None => return Ok(ToolResult::fail("Missing 'trigger_id' for trigger_delete")),
        };

        let path = format!("/trigger_instances/manage/{}", trigger_id);
        let resp = self.api_request_v31(Method::DELETE, &path, &[], None).await?;
        if !is_success(resp.status) {
            return Ok(ToolResult::fail(format!(
                "Composio trigger delete failed (HTTP {}): {}",
                resp.status,
                composio_error_message(resp.body_json.as_ref(), &resp.body_text)
            )));
        }

        Ok(ToolResult::ok(format!(
            "Deleted trigger '{}'.\n\n{}",
            trigger_id,
            pretty_json_or_text(resp.body_json, resp.body_text)
        )))
    }

    async fn connect_account(&self, args: &Value) -> Result<ToolResult> {
        let user_id = args
            .get("user_id")
            .or(args.get("entity_id"))
            .and_then(|v| v.as_str())
            .unwrap_or(&self.user_id);
        let toolkit_slug = toolkit_from_args(args);
        let explicit_auth_config_id = args
            .get("auth_config_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(|id| id.to_string());

        if let Some(auth_config_id) = explicit_auth_config_id {
            if self.get_auth_config(&auth_config_id).await?.is_none() {
                let candidates = self.discover_auth_configs(toolkit_slug.as_deref()).await?;
                return Ok(ToolResult::fail(format!(
                    "Composio auth_config_id '{}' was not found for this API key/project.{}",
                    auth_config_id,
                    render_auth_config_candidates(&candidates)
                )));
            }
            return self
                .create_link_session(&auth_config_id, user_id, args, toolkit_slug.as_deref())
                .await;
        }

        let Some(mut slug) = toolkit_slug else {
            return Ok(ToolResult::fail(
                "Connect requires either 'auth_config_id' or app/toolkit_slug (for example app='gmail').",
            ));
        };

        let mut candidates = self.discover_auth_configs(Some(&slug)).await?;

        // Fallback 1: If no candidates, try to find the correct toolkit slug via discovery
        if candidates.is_empty() {
            if let Some(actual_slug) = self.find_toolkit_slug(&slug).await? {
                if actual_slug != slug {
                    slug = actual_slug;
                    candidates = self.discover_auth_configs(Some(&slug)).await?;
                }
            }
        }

        // Fallback 2: If still no candidates, try to create a managed auth config automatically
        if candidates.is_empty() {
            if let Some(new_cfg) = self.create_managed_auth_config(&slug).await? {
                candidates.push(new_cfg);
            }
        }

        let Some(best) = choose_best_auth_config(&candidates) else {
            return Ok(ToolResult::fail(format!(
                "No auth configs found for toolkit '{}' and I couldn't create a default one. Please go to the Composio dashboard to enable this app manually.",
                slug
            )));
        };

        self.create_link_session(&best.id, user_id, args, Some(&slug))
            .await
    }

    async fn find_toolkit_slug(&self, query: &str) -> Result<Option<String>> {
        let params = vec![("query", query.to_string()), ("limit", "1".to_string())];
        let resp = self
            .api_request(Method::GET, "/tools", &params, None)
            .await?;
        if !is_success(resp.status) {
            return Ok(None);
        }

        let Some(items) = resp.body_json.as_ref().and_then(extract_array_entries) else {
            return Ok(None);
        };

        if let Some(item) = items.first() {
            return Ok(item
                .get("toolkit_slug")
                .or_else(|| {
                    item.get("toolkit")
                        .and_then(|v| v.as_object())
                        .and_then(|o| o.get("slug"))
                })
                .and_then(Value::as_str)
                .map(|s| s.to_string()));
        }
        Ok(None)
    }

    async fn create_managed_auth_config(&self, toolkit: &str) -> Result<Option<AuthConfigSummary>> {
        // Try uppercase first as some older/inconsistent slugs prefer it
        let payload = json!({
            "toolkit": toolkit.to_ascii_uppercase(),
            "name": format!("Managed {} Config", toolkit),
            "type": "use_composio_managed_auth"
        });

        let mut resp = self
            .api_request(Method::POST, "/auth_configs", &[], Some(&payload))
            .await?;

        if !is_success(resp.status) {
            // Try lowercase if uppercase failed
            let payload_lower = json!({
                "toolkit": toolkit.to_ascii_lowercase(),
                "name": format!("Managed {} Config", toolkit),
                "type": "use_composio_managed_auth"
            });
            resp = self
                .api_request(Method::POST, "/auth_configs", &[], Some(&payload_lower))
                .await?;
        }

        if !is_success(resp.status) {
            return Ok(None);
        }

        Ok(resp.body_json.as_ref().and_then(parse_auth_config))
    }

    async fn create_link_session(
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

        let resp = self
            .api_request(Method::POST, "/connected_accounts/link", &[], Some(&body))
            .await?;
        if !is_success(resp.status) {
            let base_error = format!(
                "Composio connect failed (HTTP {}): {}",
                resp.status,
                composio_error_message(resp.body_json.as_ref(), &resp.body_text)
            );

            if matches!(resp.status, 400 | 404 | 422) {
                let candidates = self.discover_auth_configs(toolkit_slug).await?;
                return Ok(ToolResult::fail(format!(
                    "{}{}",
                    base_error,
                    render_auth_config_candidates(&candidates)
                )));
            }

            return Ok(ToolResult::fail(base_error));
        }

        if let Some(ref v) = resp.body_json
            && let Some(url) = extract_auth_url(v)
        {
            return Ok(ToolResult::ok(format!(
                "Open this URL to authenticate {}: {}\n\n{}",
                user_id,
                url,
                pretty_json_or_text(resp.body_json, resp.body_text)
            )));
        }

        Ok(ToolResult::ok(pretty_json_or_text(
            resp.body_json,
            resp.body_text,
        )))
    }

    async fn discover_auth_configs(
        &self,
        toolkit_slug: Option<&str>,
    ) -> Result<Vec<AuthConfigSummary>> {
        let mut query = vec![("limit", "100".to_string())];
        if let Some(slug) = toolkit_slug {
            query.push(("toolkit_slug", slug.to_string()));
            query.push(("query", slug.to_string()));
        }
        let resp = self
            .api_request(Method::GET, "/auth_configs", &query, None)
            .await?;
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

    async fn get_auth_config(&self, auth_config_id: &str) -> Result<Option<AuthConfigSummary>> {
        let path = format!("/auth_configs/{}", auth_config_id);
        let resp = self.api_request(Method::GET, &path, &[], None).await?;
        if resp.status == 404 {
            return Ok(None);
        }
        if !is_success(resp.status) {
            return Ok(None);
        }
        Ok(resp.body_json.as_ref().and_then(parse_auth_config))
    }

    async fn discover_connected_accounts(
        &self,
        toolkit_slug: Option<&str>,
        user_id: &str,
    ) -> Result<Vec<ConnectedAccountSummary>> {
        let mut query = vec![("limit", "100".to_string()), ("user_id", user_id.to_string())];
        if let Some(slug) = toolkit_slug {
            query.push(("toolkit_slug", slug.to_string()));
            query.push(("query", slug.to_string()));
        }

        let resp = self
            .api_request(Method::GET, "/connected_accounts", &query, None)
            .await?;
        if !is_success(resp.status) {
            return Ok(Vec::new());
        }

        let Some(json_body) = resp.body_json else {
            return Ok(Vec::new());
        };

        let Some(items) = extract_array_entries(&json_body) else {
            return Ok(Vec::new());
        };

        let mut accounts: Vec<ConnectedAccountSummary> = items
            .iter()
            .filter_map(parse_connected_account)
            .collect();
        if let Some(slug) = toolkit_slug {
            accounts.retain(|a| a.toolkit_slug.as_deref() == Some(slug));
        }
        accounts.sort_by_key(connected_account_sort_key);
        Ok(accounts)
    }

    async fn tool_router_action(&self, args: &Value) -> Result<ToolResult> {
        let user_id = args
            .get("user_id")
            .or(args.get("entity_id"))
            .and_then(|v| v.as_str())
            .unwrap_or(&self.user_id);

        let toolkits = args.get("toolkits").and_then(Value::as_array).map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        });

        let tools = args.get("tools").and_then(Value::as_array).map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        });

        let tags = args.get("tags").and_then(Value::as_array).map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        });

        let mut session_payload = json!({
            "user_id": user_id,
        });
        if let Some(tk) = toolkits {
            session_payload["toolkits"] = json!({ "enabled": tk });
        }
        if let Some(t) = tools {
            let toolkit = toolkit_from_args(args);
            if let Some(toolkit) = toolkit {
                session_payload["tools"] = json!({
                    toolkit: { "enabled": t }
                });
            } else {
                session_payload["preload"] = json!({ "tools": t });
            }
        }
        if let Some(tag) = tags {
            session_payload["tags"] = json!({ "enabled": tag });
        }
        if let Some(toolkit) = toolkit_from_args(args) {
            if let Some(auth_config_id) = args.get("auth_config_id").and_then(Value::as_str) {
                session_payload["auth_configs"] = json!({
                    toolkit.clone(): auth_config_id
                });
            }
            if let Some(account_id) = args.get("connected_account_id").and_then(Value::as_str) {
                session_payload["connected_accounts"] = json!({
                    toolkit: [account_id]
                });
            }
        }

        let session_resp = self
            .api_request_v31(
                Method::POST,
                "/tool_router/session",
                &[],
                Some(&session_payload),
            )
            .await?;
        if !is_success(session_resp.status) {
            return Ok(ToolResult::fail(format!(
                "Composio failed to create tool router session (HTTP {}): {}",
                session_resp.status,
                composio_error_message(session_resp.body_json.as_ref(), &session_resp.body_text)
            )));
        }

        let session_id = match session_resp
            .body_json
            .as_ref()
            .and_then(|v| v.get("id").or(v.get("session_id")).and_then(Value::as_str))
        {
            Some(id) => id,
            None => {
                return Ok(ToolResult::fail(format!(
                    "Composio session created but no ID returned: {}",
                    session_resp.body_text
                )));
            }
        };

        let tools_path = format!("/tool_router/session/{}/tools", session_id);
        let tools_resp = self
            .api_request_v31(
                Method::GET,
                &tools_path,
                &[("limit", "100".to_string())],
                None,
            )
            .await?;

        let mut output = format!("Created Composio Tool Router Session: {}\n", session_id);

        // Handle auth requirements if returned
        if let Some(json_body) = &session_resp.body_json {
            if let Some(auth_reqs) = json_body.get("auth_requirements").and_then(Value::as_array) {
                if !auth_reqs.is_empty() {
                    output.push_str("\n⚠️ Authentication Required for this session:\n");
                    for req in auth_reqs {
                        if let Some(url) = extract_auth_url(req) {
                            let toolkit = req
                                .get("toolkit")
                                .and_then(Value::as_str)
                                .unwrap_or("toolkit");
                            output.push_str(&format!("- Please connect {}: {}\n", toolkit, url));
                        }
                    }
                    output.push('\n');
                }
            } else if let Some(url) = extract_auth_url(json_body) {
                output.push_str(&format!(
                    "\n⚠️ Authentication Required: Please visit {}\n\n",
                    url
                ));
            }
        }

        if let Some(body) = tools_resp.body_json {
            output.push_str("\nAvailable tools in this session:\n");
            output.push_str(&render_list_output(&body, None));
            output.push_str(&format!("\nTo execute inside this session, use action='execute' with session_id='{}' and tool_slug.\n", session_id));
        } else {
            output.push_str(&tools_resp.body_text);
        }

        Ok(ToolResult::ok(output))
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

fn parse_connected_account(value: &Value) -> Option<ConnectedAccountSummary> {
    let id = value
        .get("id")
        .or_else(|| value.get("connected_account_id"))
        .and_then(Value::as_str)?
        .to_string();
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
    let status = value
        .get("status")
        .or_else(|| value.get("connection_status"))
        .and_then(Value::as_str)
        .map(str::to_string);

    Some(ConnectedAccountSummary {
        id,
        toolkit_slug,
        status,
    })
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

fn connected_account_sort_key(cfg: &ConnectedAccountSummary) -> (u8, String) {
    let status_rank = match cfg.status.as_deref() {
        Some("active") | Some("ACTIVE") | Some("connected") | Some("CONNECTED") => 0,
        _ => 1,
    };
    (status_rank, cfg.id.clone())
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

fn render_connected_account_candidates(accounts: &[ConnectedAccountSummary]) -> String {
    if accounts.is_empty() {
        return String::new();
    }

    let mut lines = Vec::new();
    for account in accounts.iter().take(10) {
        let toolkit = account.toolkit_slug.as_deref().unwrap_or("unknown-toolkit");
        let status = account.status.as_deref().unwrap_or("unknown-status");
        lines.push(format!(
            "\n- {} ({}, {})",
            account.id, toolkit, status
        ));
    }
    if accounts.len() > 10 {
        lines.push(format!(
            "\n... and {} more connected accounts.",
            accounts.len() - 10
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

    if let Some(link_obj) = value.get("link")
        && let Some(url) = link_obj.get("url").and_then(Value::as_str)
    {
        return Some(url.to_string());
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

fn render_trigger_types_output(
    json_body: Option<&Value>,
    body_text: &str,
    toolkit_filter: Option<&str>,
) -> String {
    let Some(body) = json_body else {
        return body_text.to_string();
    };
    let Some(items) = extract_array_entries(body) else {
        return pretty_json_or_text(Some(body.clone()), body_text.to_string());
    };

    let mut lines = Vec::new();
    match toolkit_filter {
        Some(filter) => lines.push(format!(
            "Found {} Composio trigger type(s) for toolkit {}:",
            items.len(),
            filter
        )),
        None => lines.push(format!("Found {} Composio trigger type(s):", items.len())),
    }

    for item in items.iter().take(80) {
        let slug = item
            .get("slug")
            .or_else(|| item.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("unknown_trigger_slug");
        let toolkit = item
            .get("toolkit_slug")
            .and_then(Value::as_str)
            .or_else(|| {
                item.get("toolkit")
                    .and_then(|v| v.get("slug"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("unknown_toolkit");
        lines.push(format!("- {} ({})", slug, toolkit));
    }

    if items.len() > 80 {
        lines.push(format!(
            "... truncated {} additional trigger type(s).",
            items.len() - 80
        ));
    }
    lines.push(
        "Use action='trigger_type' with 'trigger_slug' to inspect required config and payload schema."
            .to_string(),
    );
    lines.join("\n")
}

fn render_trigger_instances_output(json_body: Option<&Value>, body_text: &str) -> String {
    let Some(body) = json_body else {
        return body_text.to_string();
    };
    let Some(items) = extract_array_entries(body) else {
        return pretty_json_or_text(Some(body.clone()), body_text.to_string());
    };

    let mut lines = vec![format!("Found {} active Composio trigger instance(s):", items.len())];
    for item in items.iter().take(80) {
        let trigger_id = item
            .get("id")
            .or_else(|| item.get("trigger_id"))
            .and_then(Value::as_str)
            .unwrap_or("unknown_trigger_id");
        let trigger_slug = item
            .get("trigger_name")
            .or_else(|| item.get("trigger_slug"))
            .and_then(Value::as_str)
            .unwrap_or("unknown_trigger_slug");
        let account_id = item
            .get("connected_account_id")
            .and_then(Value::as_str)
            .unwrap_or("unknown_account");
        let disabled = item
            .get("disabled_at")
            .and_then(|v| if v.is_null() { None } else { v.as_str() })
            .map(|_| "disabled")
            .unwrap_or("enabled");
        lines.push(format!(
            "- {} [{}] account={} status={}",
            trigger_id, trigger_slug, account_id, disabled
        ));
    }
    if items.len() > 80 {
        lines.push(format!(
            "... truncated {} additional trigger(s).",
            items.len() - 80
        ));
    }
    lines.join("\n")
}

fn pretty_json_or_text(json_body: Option<Value>, body_text: String) -> String {
    if let Some(v) = json_body {
        serde_json::to_string_pretty(&v).unwrap_or(body_text)
    } else {
        body_text
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
