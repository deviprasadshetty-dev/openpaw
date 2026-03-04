use super::{Tool, ToolResult};
use anyhow::Result;
use serde_json::Value;

/// Scout GitHub for openpaw/nullclaw/openclaw/picoclaw skills, evaluate them,
/// and return a scored list — matching Nullclaw's Scout→Evaluate pipeline.
pub struct SkillSearchTool {
    pub workspace_dir: String,
}

// ── Scoring ──────────────────────────────────────────────────────

struct Scores {
    compatibility: f64,
    quality: f64,
    security: f64,
}

impl Scores {
    /// Weighted total: compatibility 0.30, quality 0.35, security 0.35
    fn total(&self) -> f64 {
        self.compatibility * 0.30 + self.quality * 0.35 + self.security * 0.35
    }
}

const BAD_PATTERNS: &[&str] = &[
    "malware",
    "exploit",
    "crack",
    "keygen",
    "ransomware",
    "trojan",
];

fn contains_word(haystack: &str, word: &str) -> bool {
    let h = haystack.to_lowercase();
    let w = word.to_lowercase();
    let mut pos = 0;
    while let Some(i) = h[pos..].find(&w) {
        let abs = pos + i;
        let before_ok = abs == 0 || !h.as_bytes()[abs - 1].is_ascii_alphanumeric();
        let after = abs + w.len();
        let after_ok = after >= h.len() || !h.as_bytes()[after].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        pos = abs + 1;
    }
    false
}

fn score_compatibility(language: Option<&str>) -> f64 {
    match language {
        Some("Rust") | Some("Zig") => 1.0,
        Some("Python") | Some("TypeScript") | Some("JavaScript") => 0.6,
        Some(_) => 0.3,
        None => 0.2,
    }
}

fn score_quality(stars: u64) -> f64 {
    let raw = (stars as f64 + 1.0).log2() / 10.0;
    raw.min(1.0)
}

fn score_security(has_license: bool, name: &str, desc: &str) -> f64 {
    let mut score = 0.5_f64;
    if has_license {
        score += 0.3;
    }
    for pat in BAD_PATTERNS {
        if contains_word(name, pat) || contains_word(desc, pat) {
            score -= 0.5;
            break;
        }
    }
    score.clamp(0.0, 1.0)
}

fn recommendation(total: f64) -> &'static str {
    if total >= 0.7 {
        "auto"
    } else if total >= 0.4 {
        "manual"
    } else {
        "skip"
    }
}

// ── Tool impl ────────────────────────────────────────────────────

impl Tool for SkillSearchTool {
    fn name(&self) -> &str {
        "skill_search"
    }

    fn description(&self) -> &str {
        "Search GitHub for openpaw-compatible skills (also finds nullclaw/openclaw/picoclaw skills). Returns a scored list with install URLs. Use skill_install to install one."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"query":{"type":"string","description":"What kind of skill to search for, e.g. 'git deployment' or 'telegram notifications'"}},"required":["query"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim().to_string(),
            _ => return Ok(ToolResult::fail("Missing 'query' parameter")),
        };

        let candidates = match crate::skillforge::SkillForge::scout(&query) {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::fail(format!("Search failed: {}", e))),
        };

        if candidates.is_empty() {
            return Ok(ToolResult::ok(format!(
                "No skills found for '{}'. Try a broader search term.",
                query
            )));
        }

        let mut lines = vec![format!(
            "Found {} skills for '{}':\n",
            candidates.len(),
            query
        )];

        for (i, c) in candidates.iter().enumerate() {
            let compat = score_compatibility(c.language.as_deref());
            let quality = score_quality(c.stargazers_count);
            let security = score_security(
                c.has_license,
                &c.name,
                c.description.as_deref().unwrap_or(""),
            );
            let scores = Scores {
                compatibility: compat,
                quality,
                security,
            };
            let total = scores.total();
            let rec = recommendation(total);

            lines.push(format!(
                "{}. **{}** by {}\n   {}\n   ⭐ {} | Score: {:.0}% | Rec: {}\n   URL: {}\n",
                i + 1,
                c.name,
                c.owner.login,
                c.description.as_deref().unwrap_or("No description"),
                c.stargazers_count,
                total * 100.0,
                rec,
                c.html_url,
            ));
        }

        lines.push("\nTo install: use skill_install with the GitHub URL above.".to_string());
        Ok(ToolResult::ok(lines.join("\n")))
    }
}
