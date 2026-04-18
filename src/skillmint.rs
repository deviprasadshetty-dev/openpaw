//! # SkillMint — Self-Learning Skills for OpenPaw
//!
//! SkillMint watches for successful multi-step plan completions and
//! automatically distils the execution trace into a reusable skill stored
//! under `skills/minted/<slug>/`.
//!
//! ## Design principles
//! - **Conservative by default**: a skill is only minted when both a hard
//!   Rust gate AND a cheap LLM gate both agree the task was complex enough
//!   and novel enough to warrant one.
//! - **Zero primary-model cost**: all LLM calls use the configured
//!   `cheap_model` (e.g. `claude-haiku-3-5`), never the frontier model.
//! - **GitHub-backed SkillVault**: minted skills are synced to a private
//!   GitHub repo so the agent's learned knowledge survives wipes / restores.
//! - **User-explicit minting**: the user can tell the agent to save a skill
//!   directly ("remember how to do X like Y"), which bypasses the gate.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::config_types::SkillMintConfig;
use crate::skills::mint_slug;

// ── Public types ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportanceLevel {
    /// Silently auto-write; emit a brief info message to the user.
    Normal,
    /// Ask the user for confirmation before writing.
    High,
}

#[derive(Debug, Clone)]
pub enum MintResult {
    /// A skill was minted (or upgraded).
    Minted {
        slug: String,
        importance: ImportanceLevel,
        is_upgrade: bool,
    },
    /// The worthiness gate or LLM veto rejected minting.
    Skipped { reason: String },
}

// ── Distillation response from the cheap model ───────────────────

#[derive(Debug, Deserialize)]
struct DistillResponse {
    /// If false the model vetoed minting; no skill is written.
    #[serde(default)]
    mint: bool,
    /// Populated when `mint == false`.
    #[serde(default)]
    skip_reason: String,
    /// "normal" or "high" importance.
    #[serde(default = "default_importance_str")]
    importance: String,
    /// Body of SKILL.md (≤400 words).
    #[serde(default)]
    skill_md: String,
    /// TOML content for SKILL.toml.
    #[serde(default)]
    skill_toml: String,
    /// 2-3 realistic test prompts to seed evals/evals.json.
    #[serde(default)]
    eval_prompts: Vec<String>,
}

fn default_importance_str() -> String {
    "normal".to_string()
}

// ── SkillMint ────────────────────────────────────────────────────

/// The model to use for SkillMint distillation, resolved from the user's config.
///
/// Resolution order (first non-None wins):
///   1. `skillmint.distill_model` (explicit override in config)
///   2. `task_models.summarize`   (already the cheapest task model in most setups)
///   3. `task_models.subagent`    (second choice)
///   4. The agent's default model (always available)
///
/// This means users never need to set a separate model — SkillMint reuses
/// whatever secondary model they already configured.
pub fn resolve_distill_model(
    distill_model: &Option<String>,
    summarize: &Option<String>,
    subagent: &Option<String>,
    default_model: &str,
) -> String {
    distill_model
        .as_deref()
        .or(summarize.as_deref())
        .or(subagent.as_deref())
        .unwrap_or(default_model)
        .to_string()
}

pub struct SkillMint {
    workspace_dir: PathBuf,
    config: SkillMintConfig,
}

impl SkillMint {
    pub fn new(workspace_dir: PathBuf, config: SkillMintConfig) -> Self {
        Self {
            workspace_dir,
            config,
        }
    }

    // ── Stage 1: Hard worthiness gate (zero LLM cost) ─────────────

    /// Returns Some(reason) if the plan should NOT be minted, None if it passes.
    ///
    /// This runs entirely in Rust — no LLM call is made for rejected plans.
    pub fn worthiness_gate(
        &self,
        num_tasks: usize,
        all_succeeded: bool,
        duration_secs: u64,
    ) -> Option<String> {
        if !all_succeeded {
            return Some("not all tasks succeeded — partial successes teach bad patterns".into());
        }
        if num_tasks < self.config.min_tasks {
            return Some(format!(
                "only {} task(s) — need ≥{} for a complex workflow",
                num_tasks, self.config.min_tasks
            ));
        }
        if duration_secs < self.config.min_duration_secs {
            return Some(format!(
                "completed in {}s — under {}s threshold, too trivial to mint",
                duration_secs, self.config.min_duration_secs
            ));
        }
        None // passed
    }

    // ── Stage 2: Novelty check ────────────────────────────────────

    /// Returns true if a minted skill with this slug already exists AND
    /// the content hash matches (i.e. nothing new was learned).
    pub fn is_identical_to_existing(&self, slug: &str) -> bool {
        let skill_dir = self.workspace_dir.join("skills/minted").join(slug);
        skill_dir.join("SKILL.md").exists() && skill_dir.join("SKILL.toml").exists()
        // TODO: compare content hash when we have a proper lock entry
    }

    // ── Importance classification ─────────────────────────────────

    pub fn classify_importance(
        &self,
        slug: &str,
        duration_secs: u64,
        model_says_high: bool,
    ) -> ImportanceLevel {
        let is_novel = !self.is_identical_to_existing(slug);
        if duration_secs >= self.config.high_importance_secs || (is_novel && model_says_high) {
            ImportanceLevel::High
        } else {
            ImportanceLevel::Normal
        }
    }

    // ── Version management ────────────────────────────────────────

    /// Returns the next version string and archives the old SKILL.md if it exists.
    pub fn next_version(&self, slug: &str) -> String {
        let toml_path = self
            .workspace_dir
            .join("skills/minted")
            .join(slug)
            .join("SKILL.toml");

        if let Ok(content) = std::fs::read_to_string(&toml_path) {
            if let Ok(v) = toml::from_str::<toml::Value>(&content) {
                if let Some(ver) = v.get("version").and_then(|s| s.as_str()) {
                    // Parse "major.minor.patch" and bump patch
                    let parts: Vec<u32> = ver
                        .split('.')
                        .filter_map(|n| n.parse().ok())
                        .collect();
                    if parts.len() == 3 {
                        // Archive old SKILL.md
                        let md_path = self
                            .workspace_dir
                            .join("skills/minted")
                            .join(slug)
                            .join("SKILL.md");
                        let archive = md_path.with_extension(format!("md.v{}", ver));
                        let _ = std::fs::rename(&md_path, &archive);

                        return format!("{}.{}.{}", parts[0], parts[1], parts[2] + 1);
                    }
                }
            }
        }
        "0.1.0".to_string()
    }

    // ── Skill file writer ─────────────────────────────────────────

    /// Write `SKILL.toml`, `SKILL.md`, and `evals/evals.json` to disk.
    pub fn write_skill(
        &self,
        slug: &str,
        skill_toml: &str,
        skill_md: &str,
        eval_prompts: &[String],
    ) -> Result<()> {
        let skill_dir = self.workspace_dir.join("skills/minted").join(slug);
        std::fs::create_dir_all(&skill_dir)
            .with_context(|| format!("create skill dir {:?}", skill_dir))?;

        std::fs::write(skill_dir.join("SKILL.toml"), skill_toml)
            .context("write SKILL.toml")?;
        std::fs::write(skill_dir.join("SKILL.md"), skill_md)
            .context("write SKILL.md")?;

        if !eval_prompts.is_empty() {
            let evals_dir = skill_dir.join("evals");
            std::fs::create_dir_all(&evals_dir).context("create evals dir")?;
            let evals_json = serde_json::to_string_pretty(&serde_json::json!({
                "version": 1,
                "prompts": eval_prompts,
            }))?;
            std::fs::write(evals_dir.join("evals.json"), evals_json)
                .context("write evals.json")?;
        }

        info!("SkillMint: wrote skill '{}' to {:?}", slug, skill_dir);
        Ok(())
    }

    // ── Distillation prompt ───────────────────────────────────────

    /// Build the conservative distillation prompt for the cheap model.
    pub fn build_distillation_prompt(goal: &str, plan_summary: &str) -> String {
        format!(
            r#"You are an expert agent trainer. A task just completed successfully.

Goal: {goal}
Steps taken:
{plan_summary}

=== DECISION: Should this become a reusable skill? ===

A skill is worth creating ONLY if ALL of these are true:
  - The workflow is domain-specific and will recur in similar projects/repos
  - It involved non-obvious steps, specific tool sequencing, or discovered gotchas
  - Another capable agent could NOT replicate it from scratch without this guidance
  - The task required genuine problem-solving, not just lookup/retrieval

Do NOT mint a skill if ANY of these apply:
  - The task was conversational ("explain X", "translate this", "what time is it")
  - The task was one-off or person-specific ("send John a message", "fix MY typo")
  - The plan used only generic tools with no domain-specific setup or config
  - A skilled developer would consider the task trivial
  - The steps are already well-known general knowledge

BE CONSERVATIVE. When in doubt, return "mint": false.
A bad skill pollutes the agent's context and causes wrong matches.
No skill at all is better than a vague or overly-broad skill.

Output ONLY valid JSON (no markdown, no code fences):
{{
  "mint": true | false,
  "skip_reason": "<why not minting — only if mint is false>",
  "importance": "normal" | "high",
  "skill_md": "<SKILL.md body — concrete steps, gotchas, expected output. Max 400 words.>",
  "skill_toml": "<complete SKILL.toml with name/version/description/author fields>",
  "eval_prompts": ["<realistic trigger prompt 1>", "<realistic trigger prompt 2>"]
}}

For skill_toml:
  - name: use kebab-case slug matching the goal
  - description: VERY specific trigger. What exact situation fires this skill?
    BAD:  "when user wants to deploy something"
    GOOD: "deploying a Rust binary as a Fly.io app with Dockerfile and fly.toml"
  - author: "openpaw-skillmint"
  - version: "0.1.0"

For importance:
  - "high" if this is the first time this exact strategy was discovered,
    or if it involved overcoming a non-obvious blocker
  - "normal" otherwise
"#,
            goal = goal,
            plan_summary = plan_summary
        )
    }

    // ── Main orchestrator (auto-mint after plan) ──────────────────

    /// Called after a plan completes. Runs the full gate → distill → write pipeline.
    ///
    /// `model` is the resolved distillation model (from `resolve_distill_model()`).
    /// The `llm_call` closure takes `(model, prompt)` and returns the model's text
    /// response. This keeps skillmint.rs decoupled from the provider layer.
    pub async fn maybe_mint<F, Fut>(
        &self,
        goal: &str,
        plan_summary: &str,
        num_tasks: usize,
        all_succeeded: bool,
        duration_secs: u64,
        model: &str,
        llm_call: F,
    ) -> MintResult
    where
        F: Fn(String, String) -> Fut,
        Fut: std::future::Future<Output = Result<String>>,
    {
        if !self.config.enabled {
            return MintResult::Skipped {
                reason: "skillmint disabled in config".into(),
            };
        }

        // Stage 1: hard gate
        if let Some(reason) = self.worthiness_gate(num_tasks, all_succeeded, duration_secs) {
            info!("SkillMint: gate rejected — {}", reason);
            return MintResult::Skipped { reason };
        }

        let slug = mint_slug(goal);
        if slug.is_empty() {
            return MintResult::Skipped {
                reason: "goal produced empty slug".into(),
            };
        }

        // Stage 2: identical novelty check (exact match = skip entirely)
        // (minor variations are allowed through for upgrade minting)
        // TODO: implement content-hash comparison; for now we always attempt distillation
        // for plans that are genuinely novel enough to pass Stage 1.

        // Stage 3: LLM distillation (resolved model from user config)
        let prompt = Self::build_distillation_prompt(goal, plan_summary);
        let raw = match llm_call(model.to_string(), prompt).await {
            Ok(r) => r,
            Err(e) => {
                warn!("SkillMint: distillation LLM call failed — {}", e);
                return MintResult::Skipped {
                    reason: format!("distillation failed: {}", e),
                };
            }
        };

        // Parse JSON response
        let resp: DistillResponse = match self.parse_distill_response(&raw) {
            Ok(r) => r,
            Err(e) => {
                warn!("SkillMint: failed to parse distillation response — {}", e);
                return MintResult::Skipped {
                    reason: format!("response parse error: {}", e),
                };
            }
        };

        // LLM veto
        if !resp.mint {
            let reason = if resp.skip_reason.is_empty() {
                "model vetoed (no reason given)".to_string()
            } else {
                format!("model vetoed: {}", resp.skip_reason)
            };
            info!("SkillMint: {}", reason);
            return MintResult::Skipped { reason };
        }

        let is_upgrade = self.is_identical_to_existing(&slug);
        let version = if is_upgrade {
            self.next_version(&slug)
        } else {
            "0.1.0".to_string()
        };

        // Patch version into the TOML if needed
        let skill_toml = patch_toml_version(&resp.skill_toml, &version);

        // Write to disk
        if let Err(e) = self.write_skill(&slug, &skill_toml, &resp.skill_md, &resp.eval_prompts) {
            warn!("SkillMint: write failed — {}", e);
            return MintResult::Skipped {
                reason: format!("write error: {}", e),
            };
        }

        let importance = self.classify_importance(
            &slug,
            duration_secs,
            resp.importance.to_lowercase() == "high",
        );

        MintResult::Minted {
            slug,
            importance,
            is_upgrade,
        }
    }

    // ── User-explicit minting (bypasses the worthiness gate) ─────

    /// Called when the user explicitly asks the agent to save a skill.
    /// e.g. "save this as a skill", "remember how to do X like Y".
    ///
    /// The worthiness gate is bypassed entirely — if the user says to save it,
    /// we save it. Importance is always High (user initiated = important).
    /// `model` is the resolved distillation model.
    pub async fn mint_explicit<F, Fut>(
        &self,
        goal: &str,
        instructions: &str,
        model: &str,
        llm_call: F,
    ) -> Result<String>
    where
        F: Fn(String, String) -> Fut,
        Fut: std::future::Future<Output = Result<String>>,
    {
        let slug = mint_slug(goal);
        if slug.is_empty() {
            anyhow::bail!("goal produced empty slug");
        }

        // Build a simpler explicit-mint prompt
        let prompt = format!(
            r#"You are an expert agent trainer. The user explicitly wants to save the following as a reusable skill.

Goal: {goal}
Instructions provided:
{instructions}

Generate ONLY valid JSON (no markdown, no code fences):
{{
  "skill_md": "<SKILL.md body — concrete steps, gotchas, expected output. Max 400 words.>",
  "skill_toml": "<complete SKILL.toml with name/version/description/author fields>",
  "eval_prompts": ["<realistic trigger prompt 1>", "<realistic trigger prompt 2>"]
}}

For skill_toml:
  - name: {slug}
  - description: specific trigger — what exact situation fires this skill?
  - author: "user-defined"
  - version: "0.1.0"
"#,
            goal = goal,
            instructions = instructions,
            slug = slug
        );

        let raw = llm_call(model.to_string(), prompt).await?;

        // Parse a simpler response (mint is always assumed true for explicit)
        #[derive(Deserialize)]
        struct ExplicitResp {
            skill_md: String,
            skill_toml: String,
            #[serde(default)]
            eval_prompts: Vec<String>,
        }

        let resp: ExplicitResp = serde_json::from_str(raw.trim())
            .with_context(|| format!("parse explicit mint response: {}", raw))?;

        let version = if self.is_identical_to_existing(&slug) {
            self.next_version(&slug)
        } else {
            "0.1.0".to_string()
        };

        let skill_toml = patch_toml_version(&resp.skill_toml, &version);
        self.write_skill(&slug, &skill_toml, &resp.skill_md, &resp.eval_prompts)?;

        Ok(slug)
    }

    // ── Helpers ───────────────────────────────────────────────────

    fn parse_distill_response(&self, raw: &str) -> Result<DistillResponse> {
        // Strip potential markdown code fences
        let clean = raw
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        serde_json::from_str(clean)
            .with_context(|| format!("distill response JSON parse failed. Raw:\n{}", raw))
    }
}

// ── Helpers ───────────────────────────────────────────────────────

/// Patch (or add) the version field in a TOML string.
fn patch_toml_version(toml_str: &str, version: &str) -> String {
    if toml_str.contains("version =") || toml_str.contains("version=") {
        // Replace existing version
        let re = regex::Regex::new(r#"version\s*=\s*"[^"]*""#).unwrap();
        re.replace(toml_str, format!(r#"version = "{}""#, version))
            .into_owned()
    } else {
        // Append version after name line if present
        let mut out = toml_str.to_string();
        if let Some(pos) = out.find('\n') {
            out.insert_str(pos + 1, &format!("version = \"{}\"\n", version));
        } else {
            out.push_str(&format!("\nversion = \"{}\"\n", version));
        }
        out
    }
}

// ── SkillVault — GitHub-backed sync ──────────────────────────────

pub struct SkillVault {
    pub minted_dir: PathBuf,
    pub repo: Option<String>,
    pub token: Option<String>,
    pub auto_create: bool,
    pub private: bool,
}

impl SkillVault {
    pub fn new(workspace_dir: &Path, config: &SkillMintConfig) -> Self {
        Self {
            minted_dir: workspace_dir.join("skills/minted"),
            repo: config.vault_repo.clone(),
            token: config.vault_token.clone(),
            auto_create: config.vault_auto_create,
            private: config.vault_private,
        }
    }

    /// Bootstrap: if vault_repo is set and skills/minted/ has no .git, clone the repo.
    /// Called once at daemon startup.
    pub async fn bootstrap(&self) -> Result<()> {
        let Some(ref repo) = self.repo else {
            return Ok(()); // No vault configured
        };

        let git_dir = self.minted_dir.join(".git");
        if git_dir.exists() {
            return Ok(()); // Already initialised
        }

        info!("SkillVault: bootstrapping from {}", repo);
        std::fs::create_dir_all(&self.minted_dir).context("create minted dir")?;

        let token = self.token.as_deref().unwrap_or("");
        let clone_url = if token.is_empty() {
            format!("https://github.com/{}.git", repo)
        } else {
            format!("https://{}@github.com/{}.git", token, repo)
        };

        let status = tokio::process::Command::new("git")
            .args(["clone", &clone_url, "."])
            .current_dir(&self.minted_dir)
            .status()
            .await
            .context("git clone")?;

        if !status.success() {
            // Clone failed (repo may not exist yet) — init an empty repo instead
            warn!(
                "SkillVault: clone from {} failed — initialising empty local repo",
                repo
            );
            self.git_init_local().await?;
        } else {
            info!("SkillVault: successfully cloned {}", repo);
        }

        Ok(())
    }

    /// Commit and push after a mint. Runs non-blocking (spawned task).
    pub async fn sync(&self, slug: &str, version: &str, summary: &str) -> Result<()> {
        let Some(_) = &self.repo else {
            return Ok(()); // No vault configured
        };

        info!("SkillVault: syncing skill '{}' v{}", slug, version);

        let commit_msg = format!("mint: {} v{} — {}", slug, version, summary);
        let dir = self.minted_dir.clone();

        // git add .
        let _ = tokio::process::Command::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status()
            .await;

        // git commit
        let _ = tokio::process::Command::new("git")
            .args(["commit", "-m", &commit_msg])
            .current_dir(&dir)
            .status()
            .await;

        // git push (best-effort — don't fail the mint if push fails)
        let push_result = tokio::process::Command::new("git")
            .args(["push", "origin", "main"])
            .current_dir(&dir)
            .status()
            .await;

        match push_result {
            Ok(s) if s.success() => info!("SkillVault: pushed '{}'", slug),
            Ok(_) => warn!("SkillVault: push returned non-zero for '{}'", slug),
            Err(e) => warn!("SkillVault: push failed for '{}': {}", slug, e),
        }

        Ok(())
    }

    async fn git_init_local(&self) -> Result<()> {
        tokio::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&self.minted_dir)
            .status()
            .await
            .context("git init")?;

        // Write a README
        let readme = "# OpenPaw SkillVault\n\nAuto-generated skills from SkillMint.\n";
        std::fs::write(self.minted_dir.join("README.md"), readme)
            .context("write README")?;

        Ok(())
    }

    /// Create the GitHub repo via API if it doesn't exist yet.
    /// This is called lazily on first push attempt.
    pub async fn ensure_repo_exists(&self) -> Result<()> {
        let Some(ref repo) = self.repo else {
            return Ok(());
        };
        let Some(ref token) = self.token else {
            warn!("SkillVault: no vault_token set — cannot create GitHub repo");
            return Ok(());
        };

        if !self.auto_create {
            return Ok(());
        }

        let repo_name = repo.split('/').last().unwrap_or("openpaw-skills");
        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "name": repo_name,
            "description": "OpenPaw SkillVault — auto-generated reusable agent skills",
            "private": self.private,
            "auto_init": true,
        });

        let resp = client
            .post("https://api.github.com/user/repos")
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "openpaw-skillmint/0.1")
            .header("Accept", "application/vnd.github+json")
            .json(&body)
            .send()
            .await
            .context("GitHub create repo API call")?;

        if resp.status().is_success() {
            info!("SkillVault: created GitHub repo '{}'", repo);
        } else if resp.status().as_u16() == 422 {
            // 422 = repo already exists — that's fine
            info!("SkillVault: repo '{}' already exists", repo);
        } else {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            warn!(
                "SkillVault: unexpected response creating repo '{}': {} — {}",
                repo, status, body
            );
        }

        Ok(())
    }
}

// ── Keyword detection for user-explicit mint requests ─────────────

/// Returns true if the message looks like an explicit skill-save request.
///
/// Examples:
///   "save this as a skill"
///   "remember how to do this"
///   "create a skill for deploying Rust"
///   "SkillMint: learn this workflow"
pub fn is_explicit_mint_request(message: &str) -> bool {
    let lower = message.to_lowercase();
    let triggers = [
        "save this as a skill",
        "save as a skill",
        "remember how to do this",
        "remember how to do that",
        "create a skill for",
        "save this workflow",
        "add this as a skill",
        "mint this skill",
        "skillmint:",
        "learn this workflow",
        "remember this task",
        "save this task as a skill",
    ];
    triggers.iter().any(|t| lower.contains(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_mint(min_tasks: usize, min_duration: u64) -> SkillMint {
        SkillMint::new(
            PathBuf::from("/tmp/test_workspace"),
            SkillMintConfig {
                enabled: true,
                min_tasks,
                min_duration_secs: min_duration,
                ..Default::default()
            },
        )
    }

    #[test]
    fn test_gate_too_few_tasks() {
        let sm = make_mint(3, 60);
        assert!(sm.worthiness_gate(2, true, 120).is_some());
    }

    #[test]
    fn test_gate_too_short() {
        let sm = make_mint(3, 60);
        assert!(sm.worthiness_gate(5, true, 30).is_some());
    }

    #[test]
    fn test_gate_failed_tasks() {
        let sm = make_mint(3, 60);
        assert!(sm.worthiness_gate(5, false, 120).is_some());
    }

    #[test]
    fn test_gate_passes() {
        let sm = make_mint(3, 60);
        assert!(sm.worthiness_gate(4, true, 90).is_none());
    }

    #[test]
    fn test_explicit_mint_detection() {
        assert!(is_explicit_mint_request("save this as a skill"));
        assert!(is_explicit_mint_request("Remember how to do this please"));
        assert!(is_explicit_mint_request("Create a skill for deploying Rust"));
        assert!(!is_explicit_mint_request("how are you today"));
        assert!(!is_explicit_mint_request("run git status"));
    }

    #[test]
    fn test_patch_toml_version() {
        let toml = r#"name = "my-skill"
version = "0.1.0"
description = "test"
"#;
        let patched = patch_toml_version(toml, "0.2.0");
        assert!(patched.contains("0.2.0"));
        assert!(!patched.contains("0.1.0"));
    }
}
