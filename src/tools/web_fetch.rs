use super::{Tool, ToolResult};
use anyhow::Result;
use reqwest::blocking::Client;
use serde_json::Value;
use std::time::Duration;

pub struct WebFetchTool {
    pub default_max_chars: usize,
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self {
            default_max_chars: 50_000,
        }
    }
}

impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch a web page and extract its text content. Converts HTML to readable text with markdown formatting."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"url":{"type":"string","description":"URL to fetch (http or https)"},"max_chars":{"type":"integer","default":50000,"description":"Maximum characters to return"}},"required":["url"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return Ok(ToolResult::fail("Missing required 'url' parameter")),
        };

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Ok(ToolResult::fail(
                "Only http:// and https:// URLs are allowed",
            ));
        }

        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_i64())
            .unwrap_or(self.default_max_chars as i64);
        let max_chars = max_chars.clamp(100, 200_000) as usize;

        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("openpaw/0.1 (web_fetch tool)")
            .build()?;

        let response = match client.get(url).send() {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::fail(format!("Fetch failed: {}", e))),
        };

        let body = match response.text() {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolResult::fail(format!(
                    "Failed to read response body: {}",
                    e
                )));
            }
        };

        let mut extracted = html_to_text(&body);

        if extracted.len() > max_chars {
            let truncated = format!(
                "{}\n\n[Content truncated at {} chars, total {} chars]",
                &extracted[..max_chars],
                max_chars,
                extracted.len()
            );
            return Ok(ToolResult::ok(truncated));
        }

        Ok(ToolResult::ok(extracted))
    }
}

pub fn html_to_text(html: &str) -> String {
    let mut buf = String::new();
    let mut i = 0;
    let bytes = html.as_bytes();

    let mut in_script = false;
    let mut in_style = false;
    let mut last_was_newline = false;
    let mut consecutive_newlines = 0;

    while i < bytes.len() {
        if bytes[i] == b'<' {
            if let Some(tag_end) = bytes[i..].iter().position(|&c| c == b'>') {
                let tag_end = i + tag_end;
                let tag_content = &bytes[i + 1..tag_end];
                let tag_content_str = String::from_utf8_lossy(tag_content).to_lowercase();
                let tag_lower = tag_content_str.split_whitespace().next().unwrap_or("");

                if tag_content.starts_with(b"/") {
                    let close_tag = String::from_utf8_lossy(&tag_content[1..]).to_lowercase();
                    let close_tag = close_tag.split_whitespace().next().unwrap_or("");
                    if close_tag == "script" {
                        in_script = false;
                    }
                    if close_tag == "style" {
                        in_style = false;
                    }
                    i = tag_end + 1;
                    continue;
                }

                if tag_lower == "script" {
                    in_script = true;
                    i = tag_end + 1;
                    continue;
                }
                if tag_lower == "style" {
                    in_style = true;
                    i = tag_end + 1;
                    continue;
                }

                if in_script || in_style {
                    i = tag_end + 1;
                    continue;
                }

                let block_tags = [
                    "p",
                    "div",
                    "section",
                    "article",
                    "main",
                    "header",
                    "footer",
                    "nav",
                    "aside",
                    "blockquote",
                    "pre",
                    "table",
                    "tr",
                    "th",
                    "td",
                    "ul",
                    "ol",
                    "dl",
                    "dt",
                    "dd",
                    "form",
                    "fieldset",
                    "figure",
                ];
                if block_tags.contains(&tag_lower) {
                    if !last_was_newline && !buf.is_empty() {
                        append_newline(&mut buf, &mut consecutive_newlines);
                        last_was_newline = true;
                    }
                }

                if tag_lower.len() == 2
                    && tag_lower.starts_with('h')
                    && tag_lower.chars().nth(1).unwrap().is_ascii_digit()
                {
                    let level = tag_lower.chars().nth(1).unwrap() as u8 - b'0';
                    if !last_was_newline && !buf.is_empty() {
                        append_newline(&mut buf, &mut consecutive_newlines);
                        last_was_newline = true;
                    }
                    buf.push_str(&"#".repeat(level as usize));
                    buf.push(' ');
                    last_was_newline = false;
                    consecutive_newlines = 0;
                }

                if tag_lower == "li" {
                    if !last_was_newline && !buf.is_empty() {
                        append_newline(&mut buf, &mut consecutive_newlines);
                    }
                    buf.push_str("- ");
                    last_was_newline = false;
                    consecutive_newlines = 0;
                }

                if tag_lower == "br" || tag_lower == "br/" {
                    append_newline(&mut buf, &mut consecutive_newlines);
                    last_was_newline = true;
                }

                if tag_lower == "hr" || tag_lower == "hr/" {
                    if !last_was_newline {
                        append_newline(&mut buf, &mut consecutive_newlines);
                    }
                    buf.push_str("---");
                    append_newline(&mut buf, &mut consecutive_newlines);
                    last_was_newline = true;
                }

                i = tag_end + 1;
                continue;
            }
        }

        if in_script || in_style {
            i += 1;
            continue;
        }

        let c = bytes[i] as char;
        if c == '\n' || c == '\r' {
            if !last_was_newline {
                buf.push(' ');
            }
        } else if c == ' ' || c == '\t' {
            if !buf.is_empty() && !buf.ends_with(' ') && !last_was_newline {
                buf.push(' ');
            }
        } else {
            buf.push(c);
            last_was_newline = false;
            consecutive_newlines = 0;
        }

        i += 1;
    }

    buf.trim_end().to_string()
}

fn append_newline(buf: &mut String, consecutive: &mut u32) {
    if *consecutive < 2 {
        buf.push('\n');
        *consecutive += 1;
    }
}
