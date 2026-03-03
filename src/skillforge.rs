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

pub struct SkillForge;

impl SkillForge {
    pub fn scout(query: &str) -> Result<Vec<SkillCandidate>> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("openpaw/0.1")
            .build()?;

        let url = format!(
            "https://api.github.com/search/repositories?q={}+topic:openpaw&sort=stars&order=desc&per_page=30",
            urlencoding::encode(query)
        );

        let response = client
            .get(&url)
            .header(header::ACCEPT, "application/vnd.github.v3+json")
            .send()?;

        if !response.status().is_success() {
            return Ok(Vec::new());
        }

        let search_results: SearchResponse = response.json()?;
        
        let candidates = search_results.items.into_iter().map(|repo| {
            SkillCandidate {
                name: repo.name,
                html_url: repo.html_url,
                description: repo.description,
                stargazers_count: repo.stargazers_count,
                language: repo.language,
                owner: repo.owner,
                has_license: repo.license.is_some(),
            }
        }).collect();

        Ok(candidates)
    }
}
