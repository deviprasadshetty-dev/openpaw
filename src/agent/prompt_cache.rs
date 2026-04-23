use crate::providers::ChatMessage;

/// Anthropic prompt caching (`system_and_3` strategy).
///
/// Reduces input token costs by ~75% on multi-turn conversations by caching
/// the conversation prefix. Uses 4 cache_control breakpoints (Anthropic max):
///   1. System prompt (stable across all turns)
///   2-4. Last 3 non-system messages (rolling window)
///
/// Pure functions — no class state, no Agent dependency.

/// Add cache_control to a single message, handling all format variations.
fn apply_cache_marker(msg: &mut ChatMessage, native_anthropic: bool) {
    if msg.role == "tool" {
        if native_anthropic {
            // For native Anthropic API, tool results can have cache_control
            // We store it in a custom field via JSON manipulation if needed,
            // but for now we skip tool results in native mode since Rust
            // ChatMessage doesn't have an extra properties map.
        }
        return;
    }

    if msg.content.is_empty() {
        // Empty content — cache the whole message
        return;
    }

    // For content-based caching, we mark the message by appending a
    // special sentinel that the Anthropic provider adapter can detect.
    // The sentinel is invisible to the model but signals where to place
    // the cache_control breakpoint.
    msg.content.push_str("\n\n[anthropic:cache_control:ephemeral]");
}

/// Apply `system_and_3` caching strategy to messages for Anthropic models.
///
/// Places up to 4 cache_control breakpoints: system prompt + last 3 non-system messages.
///
/// Returns cloned messages with cache markers injected.
pub fn apply_anthropic_cache_control(
    messages: &[ChatMessage],
    native_anthropic: bool,
) -> Vec<ChatMessage> {
    let mut result: Vec<ChatMessage> = messages.to_vec();
    if result.is_empty() {
        return result;
    }

    let mut breakpoints_used = 0u32;

    // Breakpoint 1: system prompt
    if result[0].role == "system" {
        apply_cache_marker(&mut result[0], native_anthropic);
        breakpoints_used += 1;
    }

    // Remaining breakpoints on last N non-system messages
    let remaining = 4u32 - breakpoints_used;
    let non_sys_indices: Vec<usize> = result
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role != "system")
        .map(|(i, _)| i)
        .collect();

    for &idx in non_sys_indices.iter().rev().take(remaining as usize) {
        apply_cache_marker(&mut result[idx], native_anthropic);
    }

    result
}

/// Strip cache control markers from messages (for display/logging).
pub fn strip_cache_markers(messages: &mut [ChatMessage]) {
    let marker = "\n\n[anthropic:cache_control:ephemeral]";
    for msg in messages.iter_mut() {
        if msg.content.ends_with(marker) {
            msg.content.truncate(msg.content.len() - marker.len());
        }
    }
}

/// Check if a model/provider combination supports prompt caching.
pub fn supports_prompt_caching(model: &str, provider_name: &str) -> bool {
    let model_lower = model.to_lowercase();
    let provider_lower = provider_name.to_lowercase();

    // Anthropic native and compatible endpoints
    if provider_lower == "anthropic"
        || provider_lower == "openrouter"
        || provider_lower.contains("anthropic")
    {
        // Claude 3.5 Sonnet, Claude 3 Opus, Claude 3 Haiku, and newer
        if model_lower.contains("claude-3")
            || model_lower.contains("claude-opus")
            || model_lower.contains("claude-sonnet")
            || model_lower.contains("claude-haiku")
        {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_anthropic_cache_control() {
        let messages = vec![
            ChatMessage::system("You are helpful."),
            ChatMessage::user("Hello!"),
            ChatMessage::assistant("Hi there."),
            ChatMessage::user("How are you?"),
        ];

        let cached = apply_anthropic_cache_control(&messages, false);
        assert!(cached[0].content.contains("[anthropic:cache_control:ephemeral]"));
        assert!(!cached[1].content.contains("[anthropic:cache_control:ephemeral]"));
        assert!(cached[2].content.contains("[anthropic:cache_control:ephemeral]")
            || cached[3].content.contains("[anthropic:cache_control:ephemeral]"));
    }

    #[test]
    fn test_strip_cache_markers() {
        let mut messages = vec![
            ChatMessage::system("You are helpful.\n\n[anthropic:cache_control:ephemeral]"),
        ];
        strip_cache_markers(&mut messages);
        assert!(!messages[0].content.contains("[anthropic:cache_control:ephemeral]"));
    }
}
