use serde::{Deserialize, Serialize};

pub const START_TAG: &str = "<nc_choices>";
pub const END_TAG: &str = "</nc_choices>";
pub const MAX_OPTIONS: usize = 6;
pub const MIN_OPTIONS: usize = 2;
pub const MAX_ID_LEN: usize = 24;
pub const MAX_LABEL_LEN: usize = 64;
pub const MAX_SUBMIT_TEXT_LEN: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub id: String,
    pub label: String,
    #[serde(rename = "submit_text", default)]
    pub submit_text_opt: Option<String>,
    #[serde(skip)]
    pub submit_text: String, // Resolved value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChoicesDirective {
    #[serde(rename = "v", default = "default_version")]
    pub version: u8,
    pub options: Vec<ChoiceOption>,
}

fn default_version() -> u8 {
    1
}

#[derive(Debug, Clone)]
pub struct ParsedAssistantChoices {
    pub visible_text: String,
    pub choices: Option<ChoicesDirective>,
}

struct ChoicesBlockSpan {
    open_start: usize,
    content_start: usize,
    close_start: usize,
    close_end: usize,
}

pub fn parse_assistant_choices(text: &str) -> ParsedAssistantChoices {
    let span = match find_choices_block(text) {
        Some(s) => s,
        None => {
            return ParsedAssistantChoices {
                visible_text: text.to_string(),
                choices: None,
            };
        }
    };

    let mut visible = strip_choices_block(text, &span);
    let json_payload = text[span.content_start..span.close_start].trim();

    let choices = match parse_choices_directive(json_payload) {
        Ok(c) => Some(c),
        Err(_) => {
            // Parsing failed, fallback logic
            if visible.trim().is_empty() {
                // If stripping left nothing, restore original text
                return ParsedAssistantChoices {
                    visible_text: text.to_string(),
                    choices: None,
                };
            }
            return ParsedAssistantChoices {
                visible_text: visible,
                choices: None,
            };
        }
    };

    if let Some(ref c) = choices {
        if visible.trim().is_empty() {
            visible = synthesize_fallback_text(&c.options);
        }
    }

    ParsedAssistantChoices {
        visible_text: visible,
        choices,
    }
}

fn find_choices_block(text: &str) -> Option<ChoicesBlockSpan> {
    let open_start = text.find(START_TAG)?;
    let content_start = open_start + START_TAG.len();
    let rel_close = text[content_start..].find(END_TAG)?;
    let close_start = content_start + rel_close;
    let close_end = close_start + END_TAG.len();

    Some(ChoicesBlockSpan {
        open_start,
        content_start,
        close_start,
        close_end,
    })
}

fn strip_choices_block(text: &str, span: &ChoicesBlockSpan) -> String {
    let mut out = String::with_capacity(text.len() - (span.close_end - span.open_start));
    out.push_str(&text[..span.open_start]);
    out.push_str(&text[span.close_end..]);
    out
}

fn synthesize_fallback_text(options: &[ChoiceOption]) -> String {
    let mut out = String::from("Choose: ");
    for (i, opt) in options.iter().enumerate() {
        if i > 0 {
            out.push_str(" / ");
        }
        out.push_str(&opt.label);
    }
    out
}

fn parse_choices_directive(json_payload: &str) -> Result<ChoicesDirective, ()> {
    if json_payload.is_empty() {
        return Err(());
    }

    let mut directive: ChoicesDirective = serde_json::from_str(json_payload).map_err(|_| ())?;

    if directive.version != 1 {
        return Err(());
    }

    if directive.options.len() < MIN_OPTIONS || directive.options.len() > MAX_OPTIONS {
        return Err(());
    }

    // Validation loop
    for i in 0..directive.options.len() {
        let opt = &mut directive.options[i];
        
        if !is_valid_choice_id(&opt.id) {
            return Err(());
        }
        if opt.label.is_empty() || opt.label.len() > MAX_LABEL_LEN {
            return Err(());
        }

        // Resolve submit_text
        let resolved = match &opt.submit_text_opt {
            Some(s) => s.clone(),
            None => opt.label.clone(),
        };
        
        if resolved.is_empty() || resolved.len() > MAX_SUBMIT_TEXT_LEN {
            return Err(());
        }
        opt.submit_text = resolved;
    }

    // Duplicate check
    for i in 0..directive.options.len() {
        for j in (i + 1)..directive.options.len() {
            if directive.options[i].id == directive.options[j].id {
                return Err(());
            }
        }
    }

    Ok(directive)
}

fn is_valid_choice_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_ID_LEN {
        return false;
    }
    for c in id.chars() {
        let ok = c.is_ascii_alphanumeric() || c == '_' || c == '-';
        if !ok {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_directive() {
        let text = "You did it?\n<nc_choices>\n{\"v\":1,\"options\":[{\"id\":\"yes\",\"label\":\"Da\",\"submit_text\":\"Da, sdelal\"},{\"id\":\"no\",\"label\":\"Net\"}]}\n</nc_choices>";
        let parsed = parse_assistant_choices(text);
        
        assert!(parsed.choices.is_some());
        assert_eq!(parsed.visible_text, "You did it?\n");
        let choices = parsed.choices.unwrap();
        assert_eq!(choices.options.len(), 2);
        assert_eq!(choices.options[0].id, "yes");
        assert_eq!(choices.options[0].submit_text, "Da, sdelal");
        assert_eq!(choices.options[1].submit_text, "Net"); // fallback
    }

    #[test]
    fn test_invalid_json() {
        let text = "Question\n<nc_choices>{invalid}</nc_choices>";
        let parsed = parse_assistant_choices(text);
        assert!(parsed.choices.is_none());
        assert_eq!(parsed.visible_text, "Question\n");
    }

    #[test]
    fn test_invalid_json_only() {
        let text = "<nc_choices>{invalid}</nc_choices>";
        let parsed = parse_assistant_choices(text);
        assert!(parsed.choices.is_none());
        assert_eq!(parsed.visible_text, text); // Keeps original
    }

    #[test]
    fn test_duplicates() {
        let text = "Pick\n<nc_choices>{\"v\":1,\"options\":[{\"id\":\"a\",\"label\":\"A\"},{\"id\":\"a\",\"label\":\"B\"}]}</nc_choices>";
        let parsed = parse_assistant_choices(text);
        assert!(parsed.choices.is_none());
    }

    #[test]
    fn test_fallback_synthesis() {
        let text = "<nc_choices>{\"v\":1,\"options\":[{\"id\":\"a\",\"label\":\"A\"},{\"id\":\"b\",\"label\":\"B\"}]}</nc_choices>";
        let parsed = parse_assistant_choices(text);
        assert!(parsed.choices.is_some());
        assert_eq!(parsed.visible_text, "Choose: A / B");
    }
}
