use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedToolCall {
    pub name: String,
    pub arguments_json: String,
    pub tool_call_id: Option<String>,
}

pub struct ParseResult {
    pub text: String,
    pub calls: Vec<ParsedToolCall>,
}

#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    pub name: String,
    pub output: String,
    pub success: bool,
    pub tool_call_id: Option<String>,
}

pub fn parse_tool_calls(response: &str) -> ParseResult {
    if is_native_json_format(response) {
        if let Some(result) = parse_native_tool_calls(response) {
            if !result.calls.is_empty() {
                return result;
            }
        }
    }

    parse_xml_tool_calls(response)
}

fn is_native_json_format(text: &str) -> bool {
    let trimmed = text.trim();
    if !trimmed.starts_with('{') {
        return false;
    }
    trimmed.contains("\"tool_calls\"")
}

fn parse_native_tool_calls(response: &str) -> Option<ParseResult> {
    let parsed: Value = serde_json::from_str(response).ok()?;
    let obj = parsed.as_object()?;

    let text = obj
        .get("content")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    let tool_calls_arr = obj.get("tool_calls")?.as_array()?;

    let mut calls = Vec::new();
    for tc_val in tool_calls_arr {
        let tc_obj = if let Some(o) = tc_val.as_object() {
            o
        } else {
            continue;
        };
        let func_obj = if let Some(v) = tc_obj.get("function").and_then(|v| v.as_object()) {
            v
        } else {
            continue;
        };

        let name_str = if let Some(n) = func_obj.get("name").and_then(|v| v.as_str()) {
            n
        } else {
            continue;
        };
        if name_str.is_empty() {
            continue;
        }

        let args_str = func_obj
            .get("arguments")
            .and_then(|v| v.as_str())
            .unwrap_or("{}");

        let tc_id = tc_obj
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        calls.push(ParsedToolCall {
            name: name_str.to_string(),
            arguments_json: args_str.to_string(),
            tool_call_id: tc_id,
        });
    }

    Some(ParseResult { text, calls })
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn parse_xml_tool_calls(response: &str) -> ParseResult {
    let mut text_parts = Vec::new();
    let mut calls = Vec::new();
    let mut remaining = response;

    while !remaining.is_empty() {
        let xml_start = remaining.find("<tool_call>");
        let br_start_upper = remaining.find("[TOOL_CALL]");
        let br_start_lower = remaining.find("[tool_call]");

        let mut best_start = None;
        let mut open_c = '<';

        if let Some(s) = xml_start {
            best_start = Some(s);
        }
        if let Some(s) = br_start_upper {
            if best_start.is_none() || s < best_start.unwrap() {
                best_start = Some(s);
                open_c = '[';
            }
        }
        if let Some(s) = br_start_lower {
            if best_start.is_none() || s < best_start.unwrap() {
                best_start = Some(s);
                open_c = '[';
            }
        }

        if let Some(start) = best_start {
            let before = remaining[..start].trim();
            if !before.is_empty() {
                text_parts.push(before.to_string());
            }

            let after_open = &remaining[start + 11..];
            let close_tag_start = format!("{}/", open_c);

            if let Some(close_idx) = after_open.find(&close_tag_start) {
                let inner = &after_open[..close_idx].trim();

                // Extremely basic JSON array/object extraction
                if let Some(json_start) = inner.find('{').or_else(|| inner.find('[')) {
                    let json_end = inner.rfind('}').or_else(|| inner.rfind(']'));

                    let json_slice = match json_end {
                        Some(end) if end >= json_start => &inner[json_start..=end],
                        None if inner.len() > json_start => &inner[json_start..],
                        _ => "",
                    };

                    if !json_slice.is_empty() {
                        if let Ok(Value::Object(obj)) = serde_json::from_str(json_slice) {
                            if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
                                let args = obj
                                    .get("arguments")
                                    .map(|v| {
                                        if let Value::String(s) = v {
                                            s.to_string()
                                        } else {
                                            v.to_string()
                                        }
                                    })
                                    .unwrap_or_else(|| "{}".to_string());

                                calls.push(ParsedToolCall {
                                    name: name.to_string(),
                                    arguments_json: args,
                                    tool_call_id: None,
                                });
                            }
                        }
                    }
                }

                let remaining_start = after_open[close_idx..]
                    .find('>')
                    .map(|i| close_idx + i + 1)
                    .or_else(|| after_open[close_idx..].find(']').map(|i| close_idx + i + 1))
                    .unwrap_or(close_idx);

                remaining = &after_open[remaining_start..];
            } else {
                break;
            }
        } else {
            break;
        }
    }

    let trailing = remaining.trim();
    if !trailing.is_empty() {
        text_parts.push(trailing.to_string());
    }

    ParseResult {
        text: text_parts.join("\n"),
        calls,
    }
}

pub fn format_tool_results(results: &[ToolExecutionResult]) -> String {
    let mut out = String::from("[Tool results]\n");
    for res in results {
        let status = if res.success { "ok" } else { "error" };
        out.push_str(&format!(
            "<tool_result name=\"{}\" status=\"{}\">\n{}\n</tool_result>\n",
            res.name, status, res.output
        ));
    }
    out
}

pub fn format_native_tool_results(results: &[ToolExecutionResult]) -> String {
    let mut out = String::from("[");
    for (i, res) in results.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let tc_id = res.tool_call_id.as_deref().unwrap_or("unknown");
        let content = serde_json::to_string(&res.output).unwrap_or_else(|_| "\"\"".to_string());
        out.push_str(&format!(
            "{{\"role\":\"tool\",\"tool_call_id\":\"{}\",\"content\":{}}}",
            tc_id, content
        ));
    }
    out.push(']');
    out
}
