use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AieosIdentity {
    pub identity: Option<IdentitySection>,
    pub psychology: Option<PsychologySection>,
    pub linguistics: Option<LinguisticsSection>,
    pub motivations: Option<MotivationsSection>,
    pub capabilities: Option<CapabilitiesSection>,
    pub physicality: Option<PhysicalitySection>,
    pub history: Option<HistorySection>,
    pub interests: Option<InterestsSection>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct IdentitySection {
    pub names: Option<Names>,
    pub bio: Option<String>,
    pub origin: Option<String>,
    pub residence: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Names {
    pub first: Option<String>,
    pub last: Option<String>,
    pub nickname: Option<String>,
    pub full: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PsychologySection {
    pub mbti: Option<String>,
    pub ocean: Option<OceanTraits>,
    pub moral_compass: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct OceanTraits {
    pub openness: Option<f64>,
    pub conscientiousness: Option<f64>,
    pub extraversion: Option<f64>,
    pub agreeableness: Option<f64>,
    pub neuroticism: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LinguisticsSection {
    pub style: Option<String>,
    pub formality: Option<String>,
    pub catchphrases: Option<Vec<String>>,
    pub forbidden_words: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MotivationsSection {
    pub core_drive: Option<String>,
    pub short_term_goals: Option<Vec<String>>,
    pub long_term_goals: Option<Vec<String>>,
    pub fears: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CapabilitiesSection {
    pub skills: Option<Vec<String>>,
    pub tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PhysicalitySection {
    pub appearance: Option<String>,
    pub avatar_description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct HistorySection {
    pub origin_story: Option<String>,
    pub education: Option<Vec<String>>,
    pub occupation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct InterestsSection {
    pub hobbies: Option<Vec<String>>,
    pub lifestyle: Option<String>,
}

pub fn parse_aieos_json(json_content: &str) -> serde_json::Result<AieosIdentity> {
    serde_json::from_str(json_content)
}
