use anyhow::Result;
use reqwest::{Client, header};
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
struct ClawHubSearchResponse {
    results: Vec<ClawHubSkill>,
}

#[derive(Debug, Deserialize)]
struct ClawHubSkill {
    #[serde(rename = "displayName")]
    display_name: String,
    slug: String,
    summary: String,
    score: f64,
}

#[derive(Debug, Deserialize)]
struct SkillsShSearchResponse {
    skills: Vec<SkillsShSkill>,
}

#[derive(Debug, Deserialize)]
struct SkillsShSkill {
    name: String,
    description: String,
    source: String,
    #[serde(default)]
    installs: u64,
}

pub struct SkillForge;

impl SkillForge {
    pub async fn scout(query: &str) -> Result<Vec<SkillCandidate>> {
        let mut candidates = match Self::scout_github(query).await {
            Ok(c) => c,
            Err(e) if e.to_string().contains("403") => return Err(e), // Propagate rate limit
            Err(_) => Vec::new(),
        };

        // Fallback: If github returned nothing, try with simplified query
        if candidates.is_empty() && query.contains(' ') {
            let simple_query = query.split_whitespace().next().unwrap_or(query);
            if let Ok(c) = Self::scout_github(simple_query).await {
                candidates = c;
            }
        }

        if let Ok(clawhub_candidates) = Self::scout_clawhub(query).await {
            for cand in clawhub_candidates {
                if !candidates.iter().any(|c| c.html_url == cand.html_url) {
                    candidates.push(cand);
                }
            }
        }

        if let Ok(skillssh_candidates) = Self::scout_skillssh(query).await {
            for cand in skillssh_candidates {
                if !candidates.iter().any(|c| c.html_url == cand.html_url) {
                    candidates.push(cand);
                }
            }
        }

        Ok(candidates)
    }

    async fn scout_clawhub(query: &str) -> Result<Vec<SkillCandidate>> {
        let client = Client::builder().user_agent("openpaw/0.1").build()?;

        let url = format!(
            "https://clawhub.ai/api/v1/search?q={}",
            urlencoding::encode(query)
        );

        let response = client.get(&url).send().await?;

        if !response.status().is_success() {
            return Ok(Vec::new());
        }

        let resp: ClawHubSearchResponse = response.json().await?;

        let candidates = resp
            .results
            .into_iter()
            .map(|skill| SkillCandidate {
                name: skill.display_name,
                html_url: format!("https://github.com/openclaw/skill-{}", skill.slug),
                description: Some(skill.summary),
                stargazers_count: (skill.score * 100.0) as u64,
                language: None,
                owner: Owner {
                    login: "clawhub".to_string(),
                },
                has_license: true,
            })
            .collect();

        Ok(candidates)
    }

    async fn scout_skillssh(query: &str) -> Result<Vec<SkillCandidate>> {
        let client = Client::builder().user_agent("openpaw/0.1").build()?;

        let url = format!(
            "https://skills.sh/api/search?q={}&limit=20",
            urlencoding::encode(query)
        );

        let response = client.get(&url).send().await?;

        if !response.status().is_success() {
            return Ok(Vec::new());
        }

        let resp: SkillsShSearchResponse = response.json().await?;

        let candidates = resp
            .skills
            .into_iter()
            .filter(|skill| {
                let name = skill.name.to_lowercase();
                let desc = skill.description.to_lowercase();
                name.contains("claw")
                    || name.contains("paw")
                    || desc.contains("claw")
                    || desc.contains("paw")
                    || desc.contains("agent")
            })
            .map(|skill| SkillCandidate {
                name: skill.name,
                html_url: if skill.source.starts_with("http") {
                    skill.source.clone()
                } else {
                    format!("https://github.com/{}", skill.source)
                },
                description: Some(skill.description),
                stargazers_count: skill.installs / 100,
                language: None,
                owner: Owner {
                    login: skill
                        .source
                        .split('/')
                        .next()
                        .unwrap_or("skills.sh")
                        .to_string(),
                },
                has_license: true,
            })
            .collect();

        Ok(candidates)
    }

    async fn scout_github(query: &str) -> Result<Vec<SkillCandidate>> {
        let client = Client::builder().user_agent("openpaw/0.1").build()?;

        #[derive(Debug, Deserialize)]
        struct GitHubSearchResponse {
            items: Vec<GitHubRepo>,
        }

        #[derive(Debug, Deserialize)]
        struct GitHubRepo {
            name: String,
            html_url: String,
            description: Option<String>,
            stargazers_count: u64,
            language: Option<String>,
            owner: Owner,
            license: Option<serde_json::Value>,
        }

        let topics = [
            "openpaw",
            "openclaw",
            "picoclaw",
            "ai-agent-skill",
            "agent-skill",
        ];
        let topic_filter = topics
            .iter()
            .map(|t| format!("topic:{}", t))
            .collect::<Vec<_>>()
            .join("+OR+");

        let url = format!(
            "https://api.github.com/search/repositories?q={}+({})+skill&sort=stars&order=desc&per_page=30",
            urlencoding::encode(query),
            topic_filter,
        );

        let response = client
            .get(&url)
            .header(header::ACCEPT, "application/vnd.github.v3+json")
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(anyhow::anyhow!("GitHub API rate limit exceeded. Please try again later."));
        }

        if !response.status().is_success() {
            return Ok(Vec::new());
        }

        let search_results: GitHubSearchResponse = response.json().await?;

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
