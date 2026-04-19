pub const DEFAULT_CONTEXT_TOKENS: u64 = 8192; // Assuming a reasonable default

struct ContextWindowEntry {
    key: &'static str,
    tokens: u64,
}

const MODEL_WINDOWS: &[ContextWindowEntry] = &[
    ContextWindowEntry {
        key: "gemini-3-flash-preview",
        tokens: 1_048_576,
    },
    ContextWindowEntry {
        key: "gemini-1.5-pro",
        tokens: 2_097_152,
    },
    ContextWindowEntry {
        key: "gemini-1.5-flash",
        tokens: 1_048_576,
    },
    ContextWindowEntry {
        key: "gpt-4o",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "gpt-4o-mini",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "claude-3-5-sonnet",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "claude-3.5-sonnet",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "claude-3-5-haiku",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "claude-opus-4-6",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "claude-opus-4.6",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "claude-sonnet-4-6",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "claude-sonnet-4.6",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "claude-haiku-4-5",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "gpt-5.2",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "gpt-5.2-codex",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "gpt-4.5-preview",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "gpt-4.1",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "gpt-4.1-mini",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "o3-mini",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "gemini-2.5-pro",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "gemini-2.5-flash",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "gemini-2.0-flash",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "deepseek-v3.2",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "deepseek-chat",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "deepseek-reasoner",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "llama-4-70b-instruct",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "llama-3.3-70b-versatile",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "llama-3.1-8b-instant",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "mixtral-8x7b-32768",
        tokens: 32_768,
    },
];

const PROVIDER_WINDOWS: &[ContextWindowEntry] = &[
    ContextWindowEntry {
        key: "openrouter",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "minimax",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "openai-codex",
        tokens: 200_000,
    },
    ContextWindowEntry {
        key: "moonshot",
        tokens: 256_000,
    },
    ContextWindowEntry {
        key: "kimi",
        tokens: 262_144,
    },
    ContextWindowEntry {
        key: "kimi-coding",
        tokens: 262_144,
    },
    ContextWindowEntry {
        key: "xiaomi",
        tokens: 262_144,
    },
    ContextWindowEntry {
        key: "ollama",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "lmstudio",
        tokens: 8192,
    },
    ContextWindowEntry {
        key: "qwen",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "vllm",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "github-copilot",
        tokens: 128_000,
    },
    ContextWindowEntry {
        key: "qianfan",
        tokens: 98_304,
    },
    ContextWindowEntry {
        key: "nvidia",
        tokens: 131_072,
    },
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

fn lookup_table(table: &[ContextWindowEntry], key: &str) -> Option<u64> {
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

fn infer_from_model_pattern(model_id: &str) -> Option<u64> {
    if model_id.contains("32768") {
        return Some(32_768);
    }

    if starts_with_ignore_case(model_id, "claude-") {
        return Some(200_000);
    }

    if starts_with_ignore_case(model_id, "gpt-")
        || starts_with_ignore_case(model_id, "o1")
        || starts_with_ignore_case(model_id, "o3")
    {
        return Some(128_000);
    }

    if starts_with_ignore_case(model_id, "gemini-") {
        return Some(200_000);
    }
    if starts_with_ignore_case(model_id, "deepseek-") {
        return Some(128_000);
    }
    if starts_with_ignore_case(model_id, "llama") || starts_with_ignore_case(model_id, "mixtral-") {
        return Some(128_000);
    }

    None
}

fn lookup_model_candidates(model_id_raw: &str) -> Option<u64> {
    let no_latest = strip_known_suffix(model_id_raw);
    let no_date = strip_date_suffix(no_latest);

    if let Some(n) = lookup_table(MODEL_WINDOWS, model_id_raw) {
        return Some(n);
    }
    if no_latest != model_id_raw
        && let Some(n) = lookup_table(MODEL_WINDOWS, no_latest)
    {
        return Some(n);
    }
    if no_date != no_latest
        && let Some(n) = lookup_table(MODEL_WINDOWS, no_date)
    {
        return Some(n);
    }

    infer_from_model_pattern(no_date)
        .or_else(|| infer_from_model_pattern(no_latest))
        .or_else(|| infer_from_model_pattern(model_id_raw))
}

pub fn lookup_context_tokens(model_ref_raw: &str) -> Option<u64> {
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
        if let Some(n) = lookup_table(PROVIDER_WINDOWS, nested_provider) {
            return Some(n);
        }
    }

    if let Some(last_sep) = split.model.rfind('/') {
        let leaf_model = &split.model[last_sep + 1..];
        if let Some(n) = lookup_model_candidates(leaf_model) {
            return Some(n);
        }
    }

    if let Some(provider) = split.provider
        && let Some(n) = lookup_table(PROVIDER_WINDOWS, provider)
    {
        return Some(n);
    }

    None
}

pub fn resolve_context_tokens(token_limit_override: Option<u64>, model_ref: &str) -> u64 {
    token_limit_override
        .or_else(|| lookup_context_tokens(model_ref))
        .unwrap_or(DEFAULT_CONTEXT_TOKENS)
}

pub fn is_small_model_context(token_limit: u64) -> bool {
    token_limit <= 16384
}
