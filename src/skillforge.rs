use anyhow::Result;
use reqwest::header;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SkillCandidate {
    pub name: String,
    pub html_url: String,
    pub description: Option<String>,
    pub stargazers_count: u64,
    pub language: Option<String>,
    pub owner: Owner,
    #[serde(default)]
    pub has_license: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Owner {
    pub login: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    items: Vec<GitHubRepo>,
}

#[derive(Debug, Deserialize)]
struct GitHubRepo {
    name: String,
    full_name: String,
    html_url: String,
    description: Option<String>,
    stargazers_count: u64,
    language: Option<String>,
    owner: Owner,
    license: Option<License>,
}

#[derive(Debug, Deserialize)]
struct License {
    key: String,
}

#[derive(Debug, Deserialize)]
struct ClawHubSkill {
    score: f64,
    slug: String,
    displayName: String,
    summary: String,
}

pub struct SkillForge;

impl SkillForge {
    pub fn scout(query: &str) -> Result<Vec<SkillCandidate>> {
        let mut candidates = Self::scout_github(query).unwrap_or_default();
        if let Ok(clawhub_candidates) = Self::scout_clawhub(query) {
             // Merge results, avoiding duplicates by html_url
            for cand in clawhub_candidates {
                if !candidates.iter().any(|c| c.html_url == cand.html_url) {
                    candidates.push(cand);
                }
            }
        }
        
        Ok(candidates)
    }

    fn scout_clawhub(query: &str) -> Result<Vec<SkillCandidate>> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("openpaw/0.1")
            .build()?;

        let url = format!(
            "https://clawhub.ai/api/search?q={}",
            urlencoding::encode(query)
        );

        let response = client.get(&url).send()?;

        if !response.status().is_success() {
             return Ok(Vec::new());
        }

        let skills: Vec<ClawHubSkill> = response.json()?;
        
        let candidates = skills
            .into_iter()
            .map(|skill| {
                SkillCandidate {
                    name: skill.displayName,
                    html_url: format!("https://clawhub.ai/skill/{}", skill.slug),
                    description: Some(skill.summary),
                    stargazers_count: (skill.score * 100.0) as u64, // Mapping vector score to "stars" roughly
                    language: None,
                    owner: Owner { login: "clawhub".to_string() }, // Default owner since API doesn't provide it clearly in search
                    has_license: true, 
                }
            })
            .collect();

        Ok(candidates)
    }

    fn scout_github(query: &str) -> Result<Vec<SkillCandidate>> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("openpaw/0.1")
            .build()?;

        // Search all compatible skill ecosystems — openpaw is new so also pull
        // from nullclaw, openclaw, and picoclaw which already have community skills
        let topics = ["openpaw", "nullclaw", "openclaw", "picoclaw"];
        let topic_filter = topics
            .iter()
            .map(|t| format!("topic:{}", t))
            .collect::<Vec<_>>()
            .join("+OR+");

        let url = format!(
            "https://api.github.com/search/repositories?q={}+({})&sort=stars&order=desc&per_page=30",
            urlencoding::encode(query),
            topic_filter,
        );

        let response = client
            .get(&url)
            .header(header::ACCEPT, "application/vnd.github.v3+json")
            .send()?;

        if !response.status().is_success() {
            return Ok(Vec::new());
        }

        let search_results: SearchResponse = response.json()?;

        let candidates = search_results
            .items
            .into_iter()
            .map(|repo| SkillCandidate {
                name: repo.name,
                html_url: repo.html_url,
                description: repo.description,
                stargazers_count: repo.stargazers_count,
                language: repo.language,
                owner: repo.owner,
                has_license: repo.license.is_some(),
            })
            .collect();

        Ok(candidates)
    }
}
