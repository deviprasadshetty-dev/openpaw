pub struct ModelRoutingConfig {
    pub cheap_model: String,
    pub default_model: String,
}

pub fn route_to_appropriate_model<'a>(query: &str, config: &'a ModelRoutingConfig) -> &'a str {
    let lower = query.to_lowercase();

    // Greetings and acknowledgments → cheapest model
    if matches!(
        lower.trim(),
        "hi" | "hello" | "hey" | "thanks" | "ok" | "yes" | "no" | "please" | "thank you"
    ) {
        return &config.cheap_model;
    }

    // Simple factual questions or short requests → cheap model
    if query.len() < 100
        && (lower.starts_with("what is ")
            || lower.starts_with("who is ")
            || lower.starts_with("when is ")
            || lower.starts_with("where is "))
        {
            return &config.cheap_model;
        }

    // Yes/no questions → cheap model
    if (lower.starts_with("is ") || lower.starts_with("does ") || lower.starts_with("can "))
        && !lower.contains(" how ") && query.len() < 150 {
            return &config.cheap_model;
        }

    &config.default_model
}

pub fn is_greeting(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    matches!(
        lower.trim(),
        "hi" | "hello" | "hey" | "thanks" | "thank you" | "ok" | "yes" | "no"
    ) || lower.starts_with("hi ")
        || lower.starts_with("hello ")
}
