use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedToolCall {
    pub name: String,
    pub arguments_json: String,
    pub tool_call_id: Option<String>,
    pub thought_signature: Option<String>,
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
    pub thought_signature: Option<String>,
}

pub fn parse_tool_calls(response: &str) -> ParseResult {
    if is_native_json_format(response)
        && let Some(result) = parse_native_tool_calls(response)
            && !result.calls.is_empty() {
                return result;
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

        let thought_sig = tc_obj
            .get("function")
            .and_then(|v| v.as_object())
            .and_then(|f| f.get("thought_signature"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        calls.push(ParsedToolCall {
            name: name_str.to_string(),
            arguments_json: args_str.to_string(),
            tool_call_id: tc_id,
            thought_signature: thought_sig,
        });
    }

    Some(ParseResult { text, calls })
}



use regex::Regex;

fn parse_xml_tool_calls(response: &str) -> ParseResult {
    let re = Regex::new(r"(?s)(<tool_call>|\[TOOL_CALL\]|\[tool_call\])(.*?)(</tool_call>|\[/TOOL_CALL\]|\[/tool_call\])").unwrap();
    
    let mut calls = Vec::new();
    let mut last_end = 0;
    let mut text_parts = Vec::new();

    for caps in re.captures_iter(response) {
        // Text before the match
        if let Some(m) = caps.get(0) {
            if m.start() > last_end {
                text_parts.push(response[last_end..m.start()].trim().to_string());
            }
            last_end = m.end();
        }

        // The content inside the tags
        if let Some(inner) = caps.get(2) {
            let inner_str = inner.as_str().trim();
            // Try to find JSON inside (in case of extra whitespace or characters)
            if let Some(json_start) = inner_str.find('{').or_else(|| inner_str.find('[')) {
                let json_end = inner_str.rfind('}').or_else(|| inner_str.rfind(']'));
                
                if let Some(end) = json_end
                    && end >= json_start {
                        let json_slice = &inner_str[json_start..=end];
                        if let Ok(Value::Object(obj)) = serde_json::from_str(json_slice)
                             && let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
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
                                    thought_signature: None,
                                });
                            }
                    }
            }
        }
    }

    // Remaining text after the last match
    if last_end < response.len() {
        let trailing = response[last_end..].trim();
        if !trailing.is_empty() {
            text_parts.push(trailing.to_string());
        }
    }

    ParseResult {
        text: text_parts.join("\n").trim().to_string(),
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
