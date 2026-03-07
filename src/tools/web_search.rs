use super::{Tool, ToolResult};
use anyhow::Result;
use reqwest::blocking::Client;
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
            provider: "duckduckgo".to_string(),
            fallback_providers: Vec::new(),
            api_key: None,
            timeout_secs: 30,
        }
    }
}

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web. Providers: searxng, duckduckgo(ddg), brave, firecrawl, tavily, perplexity, exa, jina. Configure via http_request.search_provider/search_fallback_providers and API key env vars."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"query":{"type":"string","minLength":1,"description":"Search query"},"count":{"type":"integer","minimum":1,"maximum":10,"default":5,"description":"Number of results (1-10)"},"provider":{"type":"string","description":"Optional provider override"}},"required":["query"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q.trim(),
            None => return Ok(ToolResult::fail("Missing required 'query' parameter")),
        };

        if query.is_empty() {
            return Ok(ToolResult::fail("'query' must not be empty"));
        }

        let _count = args.get("count").and_then(|v| v.as_i64()).unwrap_or(5);
        let provider = args
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.provider);

        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()?;

        if provider == "duckduckgo" || provider == "ddg" {
            let url = format!(
                "https://html.duckduckgo.com/html/?q={}",
                urlencoding::encode(query)
            );
            let res = client
                .get(&url)
                .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
                .send();
            match res {
                Ok(resp) => {
                    let html = resp.text().unwrap_or_default();
                    if html.contains("result__snippet") {
                        Ok(ToolResult::ok(format!(
                            "Results for {}: (HTML content received from DDG)",
                            query
                        )))
                    } else {
                        Ok(ToolResult::ok(
                            "No web results found or blocked by captcha.".to_string(),
                        ))
                    }
                }
                Err(e) => Ok(ToolResult::fail(format!("DDG Search failed: {}", e))),
            }
        } else if provider == "searxng" {
            if let Some(base) = &self.searxng_base_url {
                let url = format!(
                    "{}/search?q={}&format=json",
                    base,
                    urlencoding::encode(query)
                );
                let res = client.get(&url).send();
                match res {
                    Ok(resp) => {
                        let text = resp.text().unwrap_or_default();
                        Ok(ToolResult::ok(format!("SearXNG Results:\n{}", text)))
                    }
                    Err(e) => Ok(ToolResult::fail(format!("SearXNG Search failed: {}", e))),
                }
            } else {
                Ok(ToolResult::fail("searxng_base_url not configured"))
            }
        } else if provider == "brave" {
            let api_key = match &self.api_key {
                Some(k) => k,
                None => return Ok(ToolResult::fail("Brave Search API key not configured")),
            };

            let count = args
                .get("count")
                .and_then(|v| v.as_u64())
                .unwrap_or(5)
                .min(10);
            let url = format!(
                "https://api.search.brave.com/res/v1/web/search?q={s}&count={d}",
                s = urlencoding::encode(query),
                d = count
            );

            let res = client
                .get(&url)
                .header("X-Subscription-Token", api_key)
                .header("Accept", "application/json")
                .send();

            match res {
                Ok(resp) => {
                    let json: Value = resp.json().unwrap_or(Value::Null);
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
        } else {
            Ok(ToolResult::fail(format!(
                "Provider {} not fully implemented in rust yet",
                provider
            )))
        }
    }
}
