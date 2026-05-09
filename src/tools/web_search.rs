use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

pub struct WebSearchTool {
    pub api_key: Option<String>,
    pub timeout_secs: u64,
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self {
            api_key: None,
            timeout_secs: 60,
        }
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TinyFishSearchResult {
    position: u32,
    site_name: String,
    title: String,
    snippet: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct TinyFishSearchResponse {
    query: String,
    results: Vec<TinyFishSearchResult>,
    total_results: u32,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web using TinyFish Search API. Requires a TinyFish API key configured in http_request.tinfish_api_key. Get a key at https://agent.tinyfish.ai/api-keys"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"query":{"type":"string","minLength":1,"description":"Search query"},"count":{"type":"integer","minimum":1,"maximum":20,"default":5,"description":"Number of results (1-20)"},"location":{"type":"string","description":"Country code for geo-targeted results (e.g. US, GB, FR, DE)"},"language":{"type":"string","description":"Language code for result language (e.g. en, fr, de)"},"page":{"type":"integer","minimum":0,"maximum":10,"default":0,"description":"Page number for pagination (0-based)"}},"required":["query"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q.trim(),
            None => return Ok(ToolResult::fail("Missing required 'query' parameter")),
        };

        if query.is_empty() {
            return Ok(ToolResult::fail("'query' must not be empty"));
        }

        let api_key = match &self.api_key {
            Some(k) if !k.is_empty() => k,
            _ => {
                return Ok(ToolResult::fail(
                    "TinyFish API key not configured. Get one at https://agent.tinyfish.ai/api-keys and set http_request.tinfish_api_key in config.json",
                ))
            }
        };

        let count = args
            .get("count")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .min(20);

        let location = args.get("location").and_then(|v| v.as_str());
        let language = args.get("language").and_then(|v| v.as_str());
        let page = args.get("page").and_then(|v| v.as_u64()).unwrap_or(0);

        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()?;

        let mut url = format!(
            "https://api.search.tinyfish.ai?query={}&page={}",
            urlencoding::encode(query),
            page
        );
        if let Some(loc) = location {
            url.push_str(&format!("&location={}", urlencoding::encode(loc)));
        }
        if let Some(lang) = language {
            url.push_str(&format!("&language={}", urlencoding::encode(lang)));
        }

        let res = client
            .get(&url)
            .header("X-API-Key", api_key)
            .header("Accept", "application/json")
            .send()
            .await;

        match res {
            Ok(resp) => {
                if resp.status() == 401 {
                    return Ok(ToolResult::fail(
                        "TinyFish API key invalid (401). Check your key at https://agent.tinyfish.ai/api-keys",
                    ));
                }
                if resp.status() == 429 {
                    return Ok(ToolResult::fail(
                        "TinyFish rate limit exceeded (429). Wait and retry.",
                    ));
                }
                if !resp.status().is_success() {
                    return Ok(ToolResult::fail(format!(
                        "TinyFish API returned status {}",
                        resp.status()
                    )));
                }

                let body = resp.text().await.unwrap_or_default();
                match serde_json::from_str::<TinyFishSearchResponse>(&body) {
                    Ok(data) => {
                        if data.results.is_empty() {
                            return Ok(ToolResult::ok(format!(
                                "No results found for '{}'",
                                query
                            )));
                        }

                        let mut output = format!(
                            "TinyFish Search results for '{}' ({} total):\n\n",
                            data.query, data.total_results
                        );
                        for (i, r) in data.results.iter().enumerate() {
                            if i >= count as usize {
                                break;
                            }
                            output.push_str(&format!(
                                "{}. **{}**\n   URL: {}\n   {}\n\n",
                                i + 1,
                                r.title,
                                r.url,
                                r.snippet
                            ));
                        }
                        Ok(ToolResult::ok(output))
                    }
                    Err(e) => {
                        let preview: String = body.chars().take(500).collect();
                        Ok(ToolResult::fail(format!(
                            "Failed to parse TinyFish response: {}. Preview: {}",
                            e, preview
                        )))
                    }
                }
            }
            Err(e) => Ok(ToolResult::fail(format!(
                "TinyFish search request failed: {}",
                e
            ))),
        }
    }
}
