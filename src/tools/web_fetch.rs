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

        let extracted = html_to_text(&body);

        if extracted.len() > max_chars {
            let boundary = floor_char_boundary(&extracted, max_chars);
            let truncated = format!(
                "{}\n\n[Content truncated at {} chars, total {} chars]",
                &extracted[..boundary],
                max_chars,
                extracted.len()
            );
            return Ok(ToolResult::ok(truncated));
        }

        Ok(ToolResult::ok(extracted))
    }
}

/// Walk back from `max` to the nearest char boundary in `s`.
/// Returns the largest index `<= max` that is a valid char boundary.
pub fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut idx = max;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

pub fn html_to_text(html: &str) -> String {
    let mut buf = String::new();
    let mut in_script = false;
    let mut in_style = false;
    let mut last_was_newline = false;
    let mut consecutive_newlines = 0;

    // We iterate the HTML as a &str (proper Unicode), advancing char by char.
    // When we hit '<', we do a sub-slice find on bytes for '>' since tag content
    // is always ASCII-compatible.
    let bytes = html.as_bytes();
    let mut i = 0usize; // byte index

    while i < html.len() {
        // ── Tag handling ─────────────────────────────────────────────────────
        if bytes[i] == b'<' {
            if let Some(tag_end_rel) = bytes[i..].iter().position(|&c| c == b'>') {
                let tag_end = i + tag_end_rel;
                let tag_content = &html[i + 1..tag_end];
                let tag_lower_str = tag_content.to_ascii_lowercase();
                let tag_lower = tag_lower_str.split_whitespace().next().unwrap_or("");

                if tag_content.starts_with('/') {
                    let close_tag = tag_content[1..].to_ascii_lowercase();
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
                        // last_was_newline = true; // Overwritten below
                    }
                }

                if tag_lower.len() == 2
                    && tag_lower.starts_with('h')
                    && tag_lower
                        .chars()
                        .nth(1)
                        .map(|c| c.is_ascii_digit())
                        .unwrap_or(false)
                {
                    let level = tag_lower.as_bytes()[1] - b'0';
                    if !last_was_newline && !buf.is_empty() {
                        append_newline(&mut buf, &mut consecutive_newlines);
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
            // Advance by one Unicode char so we don't get stuck inside multibyte sequences
            let ch = html[i..].chars().next().unwrap_or('\0');
            i += ch.len_utf8();
            continue;
        }

        // ── Text content ─────────────────────────────────────────────────────
        // Decode the next Unicode scalar value properly.
        let ch = match html[i..].chars().next() {
            Some(c) => c,
            None => break,
        };
        let ch_len = ch.len_utf8();

        match ch {
            '\n' | '\r' => {
                if !last_was_newline {
                    buf.push(' ');
                }
            }
            ' ' | '\t' => {
                if !buf.is_empty() && !buf.ends_with(' ') && !last_was_newline {
                    buf.push(' ');
                }
            }
            _ => {
                buf.push(ch);
                last_was_newline = false;
                consecutive_newlines = 0;
            }
        }

        i += ch_len;
    }

    buf.trim_end().to_string()
}

fn append_newline(buf: &mut String, consecutive: &mut u32) {
    if *consecutive < 2 {
        buf.push('\n');
        *consecutive += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_floor_char_boundary_ascii() {
        let s = "hello";
        assert_eq!(floor_char_boundary(s, 3), 3);
        assert_eq!(floor_char_boundary(s, 100), 5);
        assert_eq!(floor_char_boundary(s, 0), 0);
    }

    #[test]
    fn test_floor_char_boundary_multibyte() {
        // "é" is 2 bytes (0xC3 0xA9). If we ask for boundary at byte 1 we should get 0.
        let s = "éñ";
        assert_eq!(floor_char_boundary(s, 0), 0);
        assert_eq!(floor_char_boundary(s, 1), 0); // inside 'é'
        assert_eq!(floor_char_boundary(s, 2), 2); // right after 'é'
        assert_eq!(floor_char_boundary(s, 3), 2); // inside 'ñ'
        assert_eq!(floor_char_boundary(s, 4), 4); // right after 'ñ'
    }

    #[test]
    fn test_html_to_text_utf8() {
        let html = "<p>Привет мир</p><p>emoji: 🐱</p>";
        let text = html_to_text(html);
        assert!(text.contains("Привет мир"), "Cyrillic preserved: {}", text);
        assert!(text.contains('🐱'), "Emoji preserved: {}", text);
    }

    #[test]
    fn test_html_to_text_multibyte_truncation_no_panic() {
        // Build HTML with multibyte content and ensure slicing at max_chars doesn't panic.
        let html = "<p>".to_string() + &"é".repeat(60_000) + "</p>";
        let extracted = html_to_text(&html);
        // Simulated truncation: must not panic even if 50_000 is mid-codepoint
        let max_chars = 50_000;
        if extracted.len() > max_chars {
            let boundary = floor_char_boundary(&extracted, max_chars);
            let _truncated = &extracted[..boundary];
        }
        assert!(!extracted.is_empty());
    }
}
