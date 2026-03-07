pub const DEFAULT_MODEL_MAX_TOKENS: u32 = 8192;

struct MaxTokensEntry {
    key: &'static str,
    tokens: u32,
}

const MODEL_MAX_TOKENS: &[MaxTokensEntry] = &[
    MaxTokensEntry { key: "claude-opus-4-6", tokens: 8192 },
    MaxTokensEntry { key: "claude-opus-4.6", tokens: 8192 },
    MaxTokensEntry { key: "claude-sonnet-4-6", tokens: 8192 },
    MaxTokensEntry { key: "claude-sonnet-4.6", tokens: 8192 },
    MaxTokensEntry { key: "claude-haiku-4-5", tokens: 8192 },
    MaxTokensEntry { key: "gpt-5.2", tokens: 8192 },
    MaxTokensEntry { key: "gpt-5.2-codex", tokens: 8192 },
    MaxTokensEntry { key: "gpt-4.5-preview", tokens: 8192 },
    MaxTokensEntry { key: "gpt-4.1", tokens: 8192 },
    MaxTokensEntry { key: "gpt-4.1-mini", tokens: 8192 },
    MaxTokensEntry { key: "gpt-4o", tokens: 8192 },
    MaxTokensEntry { key: "gpt-4o-mini", tokens: 8192 },
    MaxTokensEntry { key: "o3-mini", tokens: 8192 },
    MaxTokensEntry { key: "gemini-2.5-pro", tokens: 8192 },
    MaxTokensEntry { key: "gemini-2.5-flash", tokens: 8192 },
    MaxTokensEntry { key: "gemini-2.0-flash", tokens: 8192 },
    MaxTokensEntry { key: "deepseek-v3.2", tokens: 8192 },
    MaxTokensEntry { key: "deepseek-chat", tokens: 8192 },
    MaxTokensEntry { key: "deepseek-reasoner", tokens: 8192 },
    MaxTokensEntry { key: "llama-4-70b-instruct", tokens: 8192 },
    MaxTokensEntry { key: "k2p5", tokens: 32_768 },
];

const PROVIDER_MAX_TOKENS: &[MaxTokensEntry] = &[
    MaxTokensEntry { key: "anthropic", tokens: 8192 },
    MaxTokensEntry { key: "openai", tokens: 8192 },
    MaxTokensEntry { key: "google", tokens: 8192 },
    MaxTokensEntry { key: "gemini", tokens: 8192 },
    MaxTokensEntry { key: "openrouter", tokens: 8192 },
    MaxTokensEntry { key: "minimax", tokens: 8192 },
    MaxTokensEntry { key: "xiaomi", tokens: 8192 },
    MaxTokensEntry { key: "moonshot", tokens: 8192 },
    MaxTokensEntry { key: "kimi", tokens: 8192 },
    MaxTokensEntry { key: "kimi-coding", tokens: 32_768 },
    MaxTokensEntry { key: "qwen", tokens: 8192 },
    MaxTokensEntry { key: "qwen-portal", tokens: 8192 },
    MaxTokensEntry { key: "ollama", tokens: 8192 },
    MaxTokensEntry { key: "vllm", tokens: 8192 },
    MaxTokensEntry { key: "github-copilot", tokens: 8192 },
    MaxTokensEntry { key: "qianfan", tokens: 32_768 },
    MaxTokensEntry { key: "nvidia", tokens: 4096 },
    MaxTokensEntry { key: "byteplus", tokens: 4096 },
    MaxTokensEntry { key: "doubao", tokens: 4096 },
    MaxTokensEntry { key: "cloudflare-ai-gateway", tokens: 64_000 },
];

fn starts_with_ignore_case(haystack: &str, prefix: &str) -> bool {
    if haystack.len() < prefix.len() {
        return false;
    }
    haystack[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn ends_with_ignore_case(haystack: &str, suffix: &str) -> bool {
    if haystack.len() < suffix.len() {
        return false;
    }
    haystack[haystack.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

fn is_all_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

fn strip_date_suffix(model_id: &str) -> &str {
    if let Some(last_dash) = model_id.rfind('-') {
        let suffix = &model_id[last_dash + 1..];
        if suffix.len() == 8 && is_all_digits(suffix) {
            return &model_id[..last_dash];
        }
    }
    model_id
}

fn strip_known_suffix(model_id: &str) -> &str {
    if ends_with_ignore_case(model_id, "-latest") {
        return &model_id[..model_id.len() - "-latest".len()];
    }
    model_id
}

fn lookup_table(table: &[MaxTokensEntry], key: &str) -> Option<u32> {
    for entry in table {
        if entry.key.eq_ignore_ascii_case(key) {
            return Some(entry.tokens);
        }
    }
    None
}

struct ProviderModelSplit<'a> {
    provider: Option<&'a str>,
    model: &'a str,
}

fn split_provider_model(model_ref: &str) -> ProviderModelSplit<'_> {
    if let Some(slash) = model_ref.find('/') {
        ProviderModelSplit {
            provider: Some(&model_ref[..slash]),
            model: &model_ref[slash + 1..],
        }
    } else {
        ProviderModelSplit {
            provider: None,
            model: model_ref,
        }
    }
}

fn infer_from_model_pattern(model_id: &str) -> Option<u32> {
    if model_id.contains("k2p5") {
        return Some(32_768);
    }
    if starts_with_ignore_case(model_id, "kimi-coding") || starts_with_ignore_case(model_id, "kimi-k2") {
        return Some(32_768);
    }
    if starts_with_ignore_case(model_id, "nvidia/") {
        return Some(4096);
    }
    if starts_with_ignore_case(model_id, "claude-")
        || starts_with_ignore_case(model_id, "gpt-")
        || starts_with_ignore_case(model_id, "o1")
        || starts_with_ignore_case(model_id, "o3")
        || starts_with_ignore_case(model_id, "gemini-")
        || starts_with_ignore_case(model_id, "deepseek-")
    {
        return Some(8192);
    }
    None
}

fn lookup_model_candidates(model_id_raw: &str) -> Option<u32> {
    let no_latest = strip_known_suffix(model_id_raw);
    let no_date = strip_date_suffix(no_latest);

    if let Some(n) = lookup_table(MODEL_MAX_TOKENS, model_id_raw) {
        return Some(n);
    }
    if no_latest != model_id_raw {
        if let Some(n) = lookup_table(MODEL_MAX_TOKENS, no_latest) {
            return Some(n);
        }
    }
    if no_date != no_latest {
        if let Some(n) = lookup_table(MODEL_MAX_TOKENS, no_date) {
            return Some(n);
        }
    }

    infer_from_model_pattern(no_date)
        .or_else(|| infer_from_model_pattern(no_latest))
        .or_else(|| infer_from_model_pattern(model_id_raw))
}

pub fn lookup_model_max_tokens(model_ref_raw: &str) -> Option<u32> {
    let model_ref = model_ref_raw.trim();
    if model_ref.is_empty() {
        return None;
    }

    if let Some(n) = lookup_model_candidates(model_ref) {
        return Some(n);
    }

    let split = split_provider_model(model_ref);
    if let Some(n) = lookup_model_candidates(split.model) {
        return Some(n);
    }

    // Support nested refs like openrouter/anthropic/claude-sonnet-4.6
    if let Some(nested_sep) = split.model.find('/') {
        let nested_provider = &split.model[..nested_sep];
        let nested_model = &split.model[nested_sep + 1..];
        if let Some(n) = lookup_model_candidates(nested_model) {
            return Some(n);
        }
        if let Some(n) = lookup_table(PROVIDER_MAX_TOKENS, nested_provider) {
            return Some(n);
        }
    }

    if let Some(last_sep) = split.model.rfind('/') {
        let leaf_model = &split.model[last_sep + 1..];
        if let Some(n) = lookup_model_candidates(leaf_model) {
            return Some(n);
        }
    }

    if let Some(provider) = split.provider {
        if let Some(n) = lookup_table(PROVIDER_MAX_TOKENS, provider) {
            return Some(n);
        }
    }

    None
}

pub fn resolve_max_tokens(max_tokens_override: Option<u32>, model_ref: &str) -> u32 {
    max_tokens_override
        .or_else(|| lookup_model_max_tokens(model_ref))
        .unwrap_or(DEFAULT_MODEL_MAX_TOKENS)
}
