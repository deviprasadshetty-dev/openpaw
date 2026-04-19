use super::{Tool, ToolContext, ToolResult, process_util};
use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

pub struct WebSearchTool {
    pub searxng_base_url: Option<String>,
    pub provider: String,
    pub fallback_providers: Vec<String>,
    pub api_key: Option<String>,
    pub timeout_secs: u64,
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self {
            searxng_base_url: None,
            provider: "gemini_cli".to_string(),
            fallback_providers: vec!["duckduckgo".to_string()],
            api_key: None,
            timeout_secs: 60,
        }
    }
}

fn is_gemini_cli_available() -> bool {
    let direct = std::process::Command::new("gemini")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if direct {
        return true;
    }

    #[cfg(windows)]
    {
        return std::process::Command::new("gemini.cmd")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
    }

    #[cfg(not(windows))]
    {
        false
    }
}

async fn search_via_gemini_cli(query: &str, count: u64) -> Result<ToolResult> {
    let prompt = format!(
        "Search the web for: {}\n\nProvide the top {} results as a numbered list. For each result include the title, URL, and a brief description. Format:\n1. **Title** - URL\n   Description",
        query,
        count.min(10)
    );

    let args = vec![
        "gemini",
        "--approval-mode",
        "yolo",
        "--output-format",
        "text",
        "-p",
        &prompt,
    ];
    let opts = process_util::RunOptions {
        timeout_ms: 60_000,
        ..Default::default()
    };

    let run_result = match process_util::run(&args, opts.clone()).await {
        Ok(res) => Ok(res),
        Err(primary_err) => {
            #[cfg(windows)]
            {
                let cmd_args = vec![
                    "gemini.cmd",
                    "--approval-mode",
                    "yolo",
                    "--output-format",
                    "text",
                    "-p",
                    &prompt,
                ];
                match process_util::run(&cmd_args, opts).await {
                    Ok(res) => Ok(res),
                    Err(_) => Err(primary_err),
                }
            }
            #[cfg(not(windows))]
            {
                Err(primary_err)
            }
        }
    };

    match run_result {
        Ok(res) if res.success => {
            let output = res.stdout.trim().to_string();
            if output.is_empty() {
                Ok(ToolResult::ok(format!(
                    "Gemini CLI search for '{}': No results returned.",
                    query
                )))
            } else {
                Ok(ToolResult::ok(format!(
                    "Gemini CLI Search results for '{}':\n\n{}",
                    query, output
                )))
            }
        }
        Ok(res) => {
            if res.stderr.contains("not found") || res.stderr.contains("command not found") {
                Ok(ToolResult::fail(
                    "Gemini CLI not found. Install it with: npm install -g @google/gemini-cli",
                ))
            } else {
                Ok(ToolResult::fail(format!(
                    "Gemini CLI search failed (exit {}): {}",
                    res.exit_code.unwrap_or(-1),
                    res.stderr
                )))
            }
        }
        Err(e) => Ok(ToolResult::fail(format!(
            "Gemini CLI not available: {}. Install with: npm install -g @google/gemini-cli",
            e
        ))),
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web using Gemini CLI (primary). Gemini CLI is free and has built-in Google Search — install with 'npm install -g @google/gemini-cli'. For image/video/audio/document analysis use the vision tool. Fallback providers: duckduckgo, searxng. Configure via http_request.search_provider."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"query":{"type":"string","minLength":1,"description":"Search query"},"count":{"type":"integer","minimum":1,"maximum":10,"default":5,"description":"Number of results (1-10)"},"provider":{"type":"string","description":"Optional provider override (gemini_cli, duckduckgo, ddg, brave, searxng)"}},"required":["query"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q.trim(),
            None => return Ok(ToolResult::fail("Missing required 'query' parameter")),
        };

        if query.is_empty() {
            return Ok(ToolResult::fail("'query' must not be empty"));
        }

        let count = args
            .get("count")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .min(10);

        let provider = args
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.provider)
            .to_string();

        let providers_to_try = {
            let mut list = vec![provider.clone()];
            for fb in &self.fallback_providers {
                if !list.contains(fb) {
                    list.push(fb.clone());
                }
            }
            list
        };

        for prov in providers_to_try {
            let result = match prov.as_str() {
                "gemini_cli" | "gemini" => {
                    if is_gemini_cli_available() {
                        search_via_gemini_cli(query, count).await
                    } else {
                        Ok(ToolResult::fail(
                            "Gemini CLI not found. Install with: npm install -g @google/gemini-cli",
                        ))
                    }
                }
                "duckduckgo" | "ddg" => self.search_duckduckgo(query, count).await,
                "brave" => self.search_brave(query, count).await,
                "searxng" => self.search_searxng(query).await,
                _ => Ok(ToolResult::fail(format!(
                    "Provider '{}' not implemented. Available: gemini_cli, duckduckgo(ddg), brave, searxng",
                    prov
                ))),
            };

            match result {
                Ok(tr) => {
                    if tr.is_error {
                        tracing::warn!("Search provider '{}' failed: {}", prov, tr.content);
                        continue;
                    }
                    return Ok(tr);
                }
                Err(e) => {
                    tracing::warn!("Search provider '{}' error: {}", prov, e);
                    continue;
                }
            }
        }

        Ok(ToolResult::fail(format!(
            "All search providers failed for '{}'. Try a different provider.",
            query
        )))
    }
}

impl WebSearchTool {
    async fn search_duckduckgo(&self, query: &str, count: u64) -> Result<ToolResult> {
        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()?;

        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(query)
        );
        let res = client
            .get(&url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .send()
            .await;

        match res {
            Ok(resp) => {
                let html = resp.text().await.unwrap_or_default();

                let mut results = Vec::new();
                let re_result = Regex::new(
                    r#"(?s)<div class="result__body">.*?<a class="result__a" href="(.*?)">(.*?)</a>.*?<span class="result__snippet">(.*?)</span>"#,
                )
                .unwrap();

                for caps in re_result.captures_iter(&html) {
                    let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let title = caps.get(2).map(|m| m.as_str()).unwrap_or("No Title");
                    let snippet = caps.get(3).map(|m| m.as_str()).unwrap_or("");

                    let title_clean = title
                        .replace("<b>", "")
                        .replace("</b>", "")
                        .replace("&amp;", "&");
                    let snippet_clean = snippet
                        .replace("<b>", "")
                        .replace("</b>", "")
                        .replace("&amp;", "&");

                    results.push(format!(
                        "- [{}]({})\n  {}\n",
                        title_clean, url, snippet_clean
                    ));
                    if results.len() >= count as usize {
                        break;
                    }
                }

                if results.is_empty() {
                    if html.contains("ddg-captcha") || html.contains("robot") {
                        Ok(ToolResult::fail(
                            "Search blocked by DuckDuckGo (captcha/bot detection). Try gemini_cli or brave.",
                        ))
                    } else {
                        Ok(ToolResult::ok(
                            "No web results found on DuckDuckGo.".to_string(),
                        ))
                    }
                } else {
                    Ok(ToolResult::ok(format!(
                        "DuckDuckGo Search results for '{}':\n\n{}",
                        query,
                        results.join("\n")
                    )))
                }
            }
            Err(e) => Ok(ToolResult::fail(format!("DDG Search failed: {}", e))),
        }
    }

    async fn search_brave(&self, query: &str, count: u64) -> Result<ToolResult> {
        let api_key = match &self.api_key {
            Some(k) if !k.is_empty() => k,
            _ => return Ok(ToolResult::fail("Brave Search API key not configured")),
        };

        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()?;

        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={s}&count={d}",
            s = urlencoding::encode(query),
            d = count.min(10)
        );

        let res = client
            .get(&url)
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .send()
            .await;

        match res {
            Ok(resp) => {
                let json: Value = resp.json().await.unwrap_or(Value::Null);
                if let Some(results) = json
                    .get("web")
                    .and_then(|w| w.get("results"))
                    .and_then(|r| r.as_array())
                {
                    if results.is_empty() {
                        return Ok(ToolResult::ok("No web results found."));
                    }

                    let mut output = format!("Brave Search results for '{}':\n\n", query);
                    for res in results {
                        let title = res
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("No Title");
                        let url = res.get("url").and_then(|v| v.as_str()).unwrap_or("#");
                        let desc = res
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        output.push_str(&format!("- [{}]({})\n  {}\n\n", title, url, desc));
                    }
                    Ok(ToolResult::ok(output))
                } else {
                    Ok(ToolResult::ok(
                        "No web results found or invalid response format.",
                    ))
                }
            }
            Err(e) => Ok(ToolResult::fail(format!("Brave Search failed: {}", e))),
        }
    }

    async fn search_searxng(&self, query: &str) -> Result<ToolResult> {
        if let Some(base) = &self.searxng_base_url {
            let client = Client::builder()
                .timeout(Duration::from_secs(self.timeout_secs))
                .build()?;
            let url = format!(
                "{}/search?q={}&format=json",
                base,
                urlencoding::encode(query)
            );
            let res = client.get(&url).send().await;
            match res {
                Ok(resp) => {
                    let text = resp.text().await.unwrap_or_default();
                    Ok(ToolResult::ok(format!("SearXNG Results:\n{}", text)))
                }
                Err(e) => Ok(ToolResult::fail(format!("SearXNG Search failed: {}", e))),
            }
        } else {
            Ok(ToolResult::fail("searxng_base_url not configured"))
        }
    }
}
