use anyhow::Result;
use reqwest::blocking::Client;
use serde_json::json;

pub trait EmbeddingProvider: Send + Sync {
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

pub struct OpenAiEmbedder {
    pub api_key: String,
    pub model: String,
    pub client: Client,
}

impl OpenAiEmbedder {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            model: "text-embedding-3-small".to_string(),
            client: Client::new(),
        }
    }
}

impl EmbeddingProvider for OpenAiEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let resp = self
            .client
            .post("https://api.openai.com/v1/embeddings")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "model": self.model,
                "input": text,
            }))
            .send()?;

        let json: serde_json::Value = resp.json()?;
        let vec = json["data"][0]["embedding"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Failed to parse OpenAI embedding response"))?
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();

        Ok(vec)
    }
}

pub struct GeminiEmbedder {
    pub api_key: String,
    pub model: String,
    pub client: Client,
}

impl GeminiEmbedder {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            model: "models/text-embedding-004".to_string(),
            client: Client::new(),
        }
    }
}

impl EmbeddingProvider for GeminiEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/{}:embedContent?key={}",
            self.model, self.api_key
        );

        let resp = self
            .client
            .post(url)
            .json(&json!({
                "model": self.model,
                "content": {
                    "parts": [{ "text": text }]
                }
            }))
            .send()?;

        let json: serde_json::Value = resp.json()?;
        let vec = json["embedding"]["values"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Failed to parse Gemini embedding response: {}", json))?
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();

        Ok(vec)
    }
}

pub struct HuggingFaceEmbedder {
    pub api_key: String,
    pub model: String,
    pub client: Client,
}

impl HuggingFaceEmbedder {
    pub fn new(api_key: &str, model: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            client: Client::new(),
        }
    }
}

impl EmbeddingProvider for HuggingFaceEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!(
            "https://api-inference.huggingface.co/pipeline/feature-extraction/{}",
            self.model
        );

        let resp = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "inputs": text,
                "options": {
                    "wait_for_model": true
                }
            }))
            .send()?;

        if !resp.status().is_success() {
            let err_text = resp.text().unwrap_or_default();
            return Err(anyhow::anyhow!("Hugging Face API error: {}", err_text));
        }

        let json: serde_json::Value = resp.json()?;

        // Hugging Face feature-extraction returns a nested array (usually [1, N, D] or [N, D] or [D])
        // For a single string input, it usually returns [D] or [[D]]
        let vec = if let Some(arr) = json.as_array() {
            if arr.is_empty() {
                return Err(anyhow::anyhow!("Empty response from Hugging Face"));
            }

            // Handle [[...]] case
            let target_arr = if arr[0].is_array() {
                arr[0].as_array().unwrap()
            } else {
                arr
            };

            target_arr
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect()
        } else {
            return Err(anyhow::anyhow!(
                "Failed to parse Hugging Face response as array"
            ));
        };

        Ok(vec)
    }
}
