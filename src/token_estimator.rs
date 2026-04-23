/// Token estimation utilities — exact port of Hermes `agent/model_metadata.py`.
///
/// Uses rough character-based estimates for pre-flight checks:
///   - ~4 chars per token (ceiling division so short texts never estimate as 0)
///   - Includes system prompt, messages, and tool schemas in request estimates
///
/// These are intentionally rough (not exact tokenizer counts) because they
/// must be fast, dependency-free, and conservative enough to trigger
/// compression before the real limit is hit.

/// Rough token estimate for a single text string.
/// Formula: `(len(text) + 3) // 4`
/// Uses ceiling division so short texts (1-3 chars) never estimate as 0 tokens.
pub fn estimate_tokens_rough(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    ((text.len() as u64) + 3) / 4
}

/// Rough token estimate for a conversation history slice.
/// Sums `len(str(msg))` for all messages, then divides by 4.
pub fn estimate_history_tokens_rough(messages: &[crate::providers::ChatMessage]) -> u64 {
    let total_chars: u64 = messages
        .iter()
        .map(|msg| {
            let mut len = msg.role.len() as u64;
            len += msg.content.len() as u64;
            if let Some(ref name) = msg.name {
                len += name.len() as u64;
            }
            if let Some(ref tcs) = msg.tool_calls {
                for tc in tcs {
                    len += tc.id.len() as u64;
                    len += tc.function.name.len() as u64;
                    len += tc.function.arguments.len() as u64;
                }
            }
            len
        })
        .sum();
    (total_chars + 3) / 4
}

/// Rough token estimate for a full chat-completions request.
/// Includes the major payload buckets: system prompt, conversation messages,
/// and tool schemas. With 50+ tools enabled, schemas alone can add 20-30K
/// tokens — a significant blind spot when only counting messages.
pub fn estimate_request_tokens_rough(
    messages: &[crate::providers::ChatMessage],
    system_prompt: &str,
    tools: Option<&[crate::providers::ToolSpec]>,
) -> u64 {
    let mut total_chars = 0u64;
    if !system_prompt.is_empty() {
        total_chars += system_prompt.len() as u64;
    }
    total_chars += messages
        .iter()
        .map(|msg| {
            let mut len = msg.role.len() as u64;
            len += msg.content.len() as u64;
            if let Some(ref name) = msg.name {
                len += name.len() as u64;
            }
            if let Some(ref tcs) = msg.tool_calls {
                for tc in tcs {
                    len += tc.id.len() as u64;
                    len += tc.function.name.len() as u64;
                    len += tc.function.arguments.len() as u64;
                }
            }
            len
        })
        .sum::<u64>();
    if let Some(tools) = tools {
        total_chars += tools
            .iter()
            .map(|t| {
                t.name.len() as u64
                    + t.description.len() as u64
                    + t.parameters.to_string().len() as u64
            })
            .sum::<u64>();
    }
    (total_chars + 3) / 4
}

/// Return model-specific pricing as (input_price_per_million, output_price_per_million).
/// Falls back to generic rates for unknown models.
pub fn get_model_prices(model: &str) -> (f64, f64) {
    let model_lower = model.to_lowercase();

    if model_lower.contains("claude-3-5-sonnet") || model_lower.contains("claude-sonnet") {
        (3.0, 15.0)
    } else if model_lower.contains("claude-3-5-haiku") || model_lower.contains("claude-haiku") {
        (0.8, 4.0)
    } else if model_lower.contains("claude-opus") {
        (15.0, 75.0)
    } else if model_lower.contains("gpt-4o-mini") {
        (0.15, 0.6)
    } else if model_lower.contains("gpt-4o") {
        (2.5, 10.0)
    } else if model_lower.contains("gpt-4.1") {
        (2.0, 8.0)
    } else if model_lower.contains("gpt-4.5") {
        (75.0, 150.0)
    } else if model_lower.contains("o3-mini") {
        (1.1, 4.4)
    } else if model_lower.contains("o1") {
        (15.0, 60.0)
    } else if model_lower.contains("gemini-1.5-flash") || model_lower.contains("gemini-2.0-flash") {
        (0.075, 0.30)
    } else if model_lower.contains("gemini-1.5-pro") || model_lower.contains("gemini-2.5-pro") {
        (1.25, 5.0)
    } else if model_lower.contains("gemini-2.5-flash") {
        (0.15, 0.60)
    } else if model_lower.contains("llama") || model_lower.contains("mixtral") {
        (0.15, 0.30)
    } else if model_lower.contains("deepseek") {
        if model_lower.contains("reasoner") {
            (0.55, 2.19)
        } else {
            (0.27, 1.10)
        }
    } else {
        (2.0, 8.0)
    }
}

/// Estimate cost in USD for a given token count.
/// Uses model-specific pricing if known, otherwise falls back to generic rates.
pub fn estimate_cost(model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    let (input_price, output_price) = get_model_prices(model);
    let input_cost = (input_tokens as f64 / 1_000_000.0) * input_price;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * output_price;
    input_cost + output_cost
}

/// Context probing tiers for when a model's context length is unknown.
/// Descending: start at 128K and step down on context-length errors.
pub const CONTEXT_PROBE_TIERS: &[u64] = &[128_000, 64_000, 32_000, 16_000, 8_000];

/// Default fallback context length when no detection method succeeds.
pub const DEFAULT_FALLBACK_CONTEXT: u64 = 128_000;

/// Minimum context length required to run the agent.
/// Models with fewer tokens cannot maintain enough working memory.
pub const MINIMUM_CONTEXT_LENGTH: u64 = 64_000;

/// Get the next lower probe tier, or None if already at minimum.
pub fn get_next_probe_tier(current_length: u64) -> Option<u64> {
    CONTEXT_PROBE_TIERS.iter().copied().find(|&t| t < current_length)
}

/// Try to extract the actual context limit from an API error message.
/// Many providers include the limit in their error text.
pub fn parse_context_limit_from_error(error_msg: &str) -> Option<u64> {
    let error_lower = error_msg.to_lowercase();
    let patterns = [
        r"(?:max(?:imum)?|limit)\s*(?:context\s*)?(?:length|size|window)?\s*(?:is|of|:)?\s*(\d{4,})",
        r"context\s*(?:length|size|window)\s*(?:is|of|:)?\s*(\d{4,})",
        r"(\d{4,})\s*(?:token)?\s*(?:context|limit)",
        r">\s*(\d{4,})\s*(?:max|limit|token)",
        r"(\d{4,})\s*(?:max(?:imum)?)\b",
    ];
    for pat in patterns {
        if let Ok(re) = regex::Regex::new(pat) {
            if let Some(cap) = re.captures(&error_lower) {
                if let Ok(limit) = cap[1].parse::<u64>() {
                    if (1024..=10_000_000).contains(&limit) {
                        return Some(limit);
                    }
                }
            }
        }
    }
    None
}

/// Detect an "output cap too large" error and return available output tokens.
/// Anthropic format: "max_tokens: 32768 > context_window: 200000 - input_tokens: 190000 = available_tokens: 10000"
pub fn parse_available_output_tokens_from_error(error_msg: &str) -> Option<u64> {
    let error_lower = error_msg.to_lowercase();
    let is_output_cap = error_lower.contains("max_tokens")
        && (error_lower.contains("available_tokens") || error_lower.contains("available tokens"));
    if !is_output_cap {
        return None;
    }
    let patterns = [
        r"available_tokens[:\s]+(\d+)",
        r"available\s+tokens[:\s]+(\d+)",
        r"=\s*(\d+)\s*$",
    ];
    for pat in patterns {
        if let Ok(re) = regex::Regex::new(pat) {
            if let Some(cap) = re.captures(&error_lower) {
                if let Ok(tokens) = cap[1].parse::<u64>() {
                    if tokens >= 1 {
                        return Some(tokens);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens_rough() {
        assert_eq!(estimate_tokens_rough(""), 0);
        assert_eq!(estimate_tokens_rough("a"), 1);
        assert_eq!(estimate_tokens_rough("abcd"), 1);
        assert_eq!(estimate_tokens_rough("abcde"), 2);
    }

    #[test]
    fn test_estimate_history_tokens_rough() {
        let messages = vec![
            crate::providers::ChatMessage::system("You are helpful."),
            crate::providers::ChatMessage::user("Hello!"),
        ];
        let tokens = estimate_history_tokens_rough(&messages);
        assert!(tokens > 0);
    }

    #[test]
    fn test_parse_context_limit_from_error() {
        assert_eq!(
            parse_context_limit_from_error("maximum context length is 32768 tokens"),
            Some(32768)
        );
        assert_eq!(
            parse_context_limit_from_error("context_length_exceeded: 131072"),
            Some(131072)
        );
    }
}
