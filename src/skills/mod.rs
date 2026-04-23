use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use tracing::warn;
use which::which;

pub mod self_improve;

fn default_version() -> String {
    "0.0.1".to_string()
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillToolDefinition {
    pub name: String,
    pub description: String,
    pub command: String,
    #[serde(default)]
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub tools: Vec<SkillToolDefinition>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub always: bool,
    #[serde(default)]
    pub requires_bins: Vec<String>,
    #[serde(default)]
    pub requires_env: Vec<String>,
    #[serde(default = "default_true")]
    pub available: bool,
    #[serde(default)]
    pub missing_deps: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub always: bool,
    #[serde(default)]
    pub requires_bins: Vec<String>,
    #[serde(default)]
    pub requires_env: Vec<String>,
}

pub fn load_skill(skill_dir: &Path) -> Result<Skill> {
    let toml_path = skill_dir.join("SKILL.toml");
    let json_path = skill_dir.join("skill.json");
    let md_path = skill_dir.join("SKILL.md");

    let instructions = fs::read_to_string(&md_path).unwrap_or_default();

    let mut skill = if toml_path.exists() {
        let content = fs::read_to_string(&toml_path).context("Failed to read SKILL.toml")?;
        let mut manifest: Skill = toml::from_str(&content).context("Failed to parse SKILL.toml")?;
        manifest.instructions = instructions;
        manifest
    } else if json_path.exists() {
        let content = fs::read_to_string(&json_path).context("Failed to read skill.json")?;
        let mut manifest: Skill =
            serde_json::from_str(&content).context("Failed to parse skill.json")?;
        manifest.instructions = instructions;
        manifest
    } else if md_path.exists() {
        let name = skill_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        Skill {
            name,
            version: "0.0.1".into(),
            description: "".into(),
            author: "".into(),
            instructions,
            enabled: true,
            always: false,
            requires_bins: vec![],
            requires_env: vec![],
            available: true,
            missing_deps: "".into(),
            path: "".into(),
            tools: vec![],
        }
    } else {
        anyhow::bail!("No skill manifest or instructions found in {:?}", skill_dir);
    };

    skill.path = skill_dir.to_string_lossy().into_owned();

    // Normalize sibling-agent names so imported skills don't confuse OpenPaw
    // about its own identity (e.g. an OpenPaw skill saying "you are OpenPaw").
    skill.name = normalize_agent_name(&skill.name);
    skill.instructions = normalize_agent_name(&skill.instructions);

    // Also normalize tool descriptions
    for t in &mut skill.tools {
        t.description = normalize_agent_name(&t.description);
    }

    Ok(skill)
}

/// Replace any occurrence of sibling agent names with "openpaw".
/// Case-insensitive, preserves surrounding text.
fn normalize_agent_name(text: &str) -> String {
    // Pairs: (pattern to match, replacement that matches the original case style)
    const NAMES: &[(&str, &str)] = &[
        ("nullclaw", "openpaw"),
        ("openclaw", "openpaw"),
        ("picoclaw", "openpaw"),
        ("NullClaw", "OpenPaw"),
        ("OpenClaw", "OpenPaw"),
        ("PicoClaw", "OpenPaw"),
        ("NULLCLAW", "OPENPAW"),
        ("OPENCLAW", "OPENPAW"),
        ("PICOCLAW", "OPENPAW"),
    ];

    let mut out = text.to_string();
    for (from, to) in NAMES {
        out = out.replace(from, to);
    }
    out
}

pub fn check_requirements(skill: &mut Skill) {
    let mut missing = Vec::new();

    for bin in &skill.requires_bins {
        if which(bin).is_err() {
            missing.push(format!("bin:{}", bin));
        }
    }

    for env in &skill.requires_env {
        if std::env::var(env).is_err() {
            missing.push(format!("env:{}", env));
        }
    }

    if !missing.is_empty() {
        skill.available = false;
        skill.missing_deps = missing.join(", ");
    } else {
        skill.available = true;
        skill.missing_deps.clear();
    }
}

pub fn list_skills(workspace_dir: &Path) -> Result<Vec<Skill>> {
    let skills_dir = workspace_dir.join("skills");
    if !skills_dir.exists() {
        return Ok(Vec::new());
    }

    let mut skills = Vec::new();
    for entry in fs::read_dir(skills_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            match load_skill(&path) {
                Ok(mut skill) => {
                    check_requirements(&mut skill);
                    skills.push(skill);
                }
                Err(e) => {
                    warn!("Skipped invalid skill in {:?}: {}", path, e);
                }
            }
        }
    }

    Ok(skills)
}

pub fn list_skills_merged(builtin_dir: &Path, workspace_dir: &Path) -> Result<Vec<Skill>> {
    let mut builtins = list_skills(builtin_dir).unwrap_or_default();
    let mut workspace = list_skills(workspace_dir).unwrap_or_default();

    // Remove builtins overridden by workspace skills of same name
    builtins.retain(|b| !workspace.iter().any(|w| w.name == b.name));

    builtins.append(&mut workspace);
    Ok(builtins)
}
