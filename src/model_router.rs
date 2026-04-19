pub struct ModelRoutingConfig {
    pub cheap_model: String,
    pub default_model: String,
}

/// Score the query from 0 (trivial) to 100 (very complex).
/// ≥ 30 → capable model; < 30 → cheap model.
pub fn complexity_score(query: &str) -> u32 {
    let lower = query.to_lowercase();
    let word_count = query.split_whitespace().count();
    let mut score: u32 = 0;

    // ── Length signal ──────────────────────────────────────────────
    score += match word_count {
        0..=3 => 0,
        4..=10 => 5,
        11..=30 => 10,
        31..=60 => 20,
        _ => 30,
    };

    // ── Code / technical signal ────────────────────────────────────
    if query.contains("```") || query.contains("`") {
        score += 15;
    }
    const CODE_KEYWORDS: &[&str] = &[
        "implement",
        "refactor",
        "debug",
        "fix the",
        "write a",
        "write code",
        "function",
        "class ",
        "struct ",
        "algorithm",
        "compile",
        "build",
        "deploy",
        "pipeline",
        "script",
        "api ",
        "endpoint",
        "database",
        "migrate",
        "optimize",
        "performance",
        "benchmark",
    ];
    for kw in CODE_KEYWORDS {
        if lower.contains(kw) {
            score += 8;
            break; // one hit is enough
        }
    }

    // ── Multi-step / planning signal ──────────────────────────────
    const PLAN_KEYWORDS: &[&str] = &[
        "step by step",
        "first.*then",
        "multiple",
        "each ",
        "for each",
        "plan ",
        "strategy",
        "roadmap",
        "schedule",
        "coordinate",
        "analyze",
        "evaluate",
        "compare",
        "research",
    ];
    for kw in PLAN_KEYWORDS {
        if lower.contains(kw) {
            score += 10;
            break;
        }
    }

    // Numbered list in query suggests multi-step intent
    let has_numbered = query.lines().any(|l| {
        let t = l.trim();
        t.starts_with("1.") || t.starts_with("1)") || t.starts_with("2.")
    });
    if has_numbered {
        score += 10;
    }

    // ── Tool-action signal ─────────────────────────────────────────
    const TOOL_KEYWORDS: &[&str] = &[
        "create",
        "generate",
        "summarize",
        "translate",
        "convert",
        "search ",
        "find ",
        "install",
        "run ",
        "execute",
        "schedule",
        "remind",
        "send ",
        "fetch ",
        "download",
        "upload",
    ];
    for kw in TOOL_KEYWORDS {
        if lower.contains(kw) {
            score += 5;
            break;
        }
    }

    // ── Cheap-signal overrides (explicit reductions) ───────────────
    const GREETINGS: &[&str] = &[
        "hi",
        "hello",
        "hey",
        "thanks",
        "thank you",
        "ok",
        "yes",
        "no",
        "please",
        "good morning",
        "good night",
    ];
    if word_count <= 4
        && GREETINGS
            .iter()
            .any(|g| lower.trim() == *g || lower.starts_with(g))
    {
        return 0;
    }

    // Simple factual/trivia questions
    if word_count < 15
        && (lower.starts_with("what is ")
            || lower.starts_with("what are ")
            || lower.starts_with("who is ")
            || lower.starts_with("when is ")
            || lower.starts_with("where is ")
            || lower.starts_with("how many ")
            || lower.starts_with("define "))
        && !lower.contains(" how ")
        && !lower.contains(" why ")
    {
        score = score.saturating_sub(15);
    }

    // Yes/no questions without complexity markers
    if (lower.starts_with("is ") || lower.starts_with("does ") || lower.starts_with("can "))
        && word_count < 12
        && score < 20
    {
        score = score.saturating_sub(10);
    }

    score.min(100)
}

pub fn route_to_appropriate_model<'a>(query: &str, config: &'a ModelRoutingConfig) -> &'a str {
    if complexity_score(query) >= 30 {
        &config.default_model
    } else {
        &config.cheap_model
    }
}

pub fn is_greeting(msg: &str) -> bool {
    complexity_score(msg) == 0
}
