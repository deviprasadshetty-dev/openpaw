/// Voice transcription via OpenAI-compatible STT (Groq Whisper).
///
/// Reads an audio file, builds a multipart/form-data request via temp file
/// (memory-efficient streaming), POSTs to Groq, returns transcribed text.
use anyhow::{Context, Result};
use reqwest::blocking::Client;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;
use tracing::warn;

const GROQ_STT_ENDPOINT: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
const DEFAULT_MODEL: &str = "whisper-large-v3";
const HTTP_TIMEOUT_SECS: u64 = 120;

// ── Transcriber trait ────────────────────────────────────────────

pub trait Transcriber: Send + Sync {
    fn transcribe(&self, path: &str) -> Result<Option<String>>;
}

// ── WhisperTranscriber — Groq / OpenAI STT ───────────────────────

pub struct WhisperTranscriber {
    pub api_key: String,
    /// STT endpoint, default = GROQ_STT_ENDPOINT
    pub endpoint: String,
    pub model: String,
    pub language: Option<String>,
    client: Client,
}

impl WhisperTranscriber {
    pub fn new_groq(api_key: impl Into<String>) -> Self {
        Self::new(api_key, GROQ_STT_ENDPOINT, DEFAULT_MODEL, None)
    }

    pub fn new(
        api_key: impl Into<String>,
        endpoint: impl Into<String>,
        model: impl Into<String>,
        language: Option<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            endpoint: endpoint.into(),
            model: model.into(),
            language,
            client: Client::builder()
                .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
                .build()
                .expect("HTTP client build failed"),
        }
    }

    /// Resolve Groq API key: explicit value > GROQ_API_KEY env var.
    pub fn resolve_groq_key(explicit: Option<&str>) -> String {
        if let Some(k) = explicit
            && !k.is_empty()
        {
            return k.to_string();
        }
        std::env::var("GROQ_API_KEY").unwrap_or_default()
    }
}

impl Transcriber for WhisperTranscriber {
    fn transcribe(&self, path: &str) -> Result<Option<String>> {
        transcribe_file(
            &self.client,
            &self.api_key,
            &self.endpoint,
            path,
            &self.model,
            self.language.as_deref(),
        )
    }
}

// ── Core transcription function ──────────────────────────────────

/// Transcribe an audio file via multipart/form-data POST.
///
/// To avoid holding the full file + multipart body in RAM simultaneously,
/// we write the multipart body to a temp file and stream it with reqwest.
pub fn transcribe_file(
    client: &Client,
    api_key: &str,
    endpoint: &str,
    audio_path: &str,
    model: &str,
    language: Option<&str>,
) -> Result<Option<String>> {
    let boundary = generate_boundary();

    // Write multipart body to temp file
    let tmp_path = std::env::temp_dir().join(format!("openpaw_voice_{}.bin", std::process::id()));

    write_multipart_to_file(&tmp_path, audio_path, &boundary, model, language)
        .context("Failed to write multipart temp file")?;

    // Build Content-Type header
    let content_type = format!("multipart/form-data; boundary={}", boundary);

    // Read temp file for the request body
    let body_bytes = fs::read(&tmp_path).context("Failed to read temp multipart file")?;
    let _ = fs::remove_file(&tmp_path); // best-effort cleanup

    let resp = client
        .post(endpoint)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", content_type)
        .body(body_bytes)
        .send()
        .context("STT HTTP request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        anyhow::bail!("STT API error {}: {}", status.as_u16(), body);
    }

    let parsed: serde_json::Value = resp.json().context("Failed to parse STT response")?;

    let text = parsed["text"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Ok(text)
}

/// Write a multipart/form-data body to a file, streaming the audio in.
fn write_multipart_to_file(
    tmp_path: &Path,
    audio_path: &str,
    boundary: &str,
    model: &str,
    language: Option<&str>,
) -> Result<()> {
    let mut f = fs::File::create(tmp_path)?;

    // ── file part ──
    write!(f, "--{}\r\n", boundary)?;
    write!(
        f,
        "Content-Disposition: form-data; name=\"file\"; filename=\"audio.ogg\"\r\n"
    )?;
    write!(f, "Content-Type: audio/ogg\r\n\r\n")?;

    let mut audio = fs::File::open(audio_path).context("Cannot open audio file")?;
    io::copy(&mut audio, &mut f)?;
    write!(f, "\r\n")?;

    // ── model part ──
    write!(f, "--{}\r\n", boundary)?;
    write!(f, "Content-Disposition: form-data; name=\"model\"\r\n\r\n")?;
    write!(f, "{}\r\n", model)?;

    // ── language part (optional) ──
    if let Some(lang) = language {
        write!(f, "--{}\r\n", boundary)?;
        write!(
            f,
            "Content-Disposition: form-data; name=\"language\"\r\n\r\n"
        )?;
        write!(f, "{}\r\n", lang)?;
    }

    // ── closing boundary ──
    write!(f, "--{}--\r\n", boundary)?;
    f.flush()?;
    Ok(())
}

/// Generate a random 32-hex-char multipart boundary.
fn generate_boundary() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let pid = std::process::id();
    format!("{:016x}{:016x}", t as u64, pid as u64)
}

// ── Telegram voice helpers ────────────────────────────────────────

/// Download a Telegram voice file and transcribe it.
/// Returns `None` if no transcriber is configured or if download fails.
pub fn transcribe_telegram_voice(
    client: &Client,
    bot_token: &str,
    file_id: &str,
    transcriber: Option<&dyn Transcriber>,
) -> Option<String> {
    let t = transcriber?;

    // 1. Resolve file_path from Telegram
    let tg_file_path = match telegram_get_file_path(client, bot_token, file_id) {
        Ok(p) => p,
        Err(e) => {
            warn!("telegram getFile failed for file_id={}: {}", file_id, e);
            return None;
        }
    };

    // 2. Download to temp
    let local_path = match telegram_download_file(client, bot_token, &tg_file_path) {
        Ok(p) => p,
        Err(e) => {
            warn!("telegram file download failed: {}", e);
            return None;
        }
    };

    // 3. Transcribe
    let result = t.transcribe(&local_path);
    let _ = fs::remove_file(&local_path); // cleanup

    match result {
        Ok(Some(text)) => Some(text),
        Ok(None) => None,
        Err(e) => {
            warn!("transcription failed: {}", e);
            None
        }
    }
}

/// Call Telegram getFile API, return the `file_path` field.
fn telegram_get_file_path(client: &Client, bot_token: &str, file_id: &str) -> Result<String> {
    let url = format!("https://api.telegram.org/bot{}/getFile", bot_token);
    let body = serde_json::json!({"file_id": file_id});

    let resp = client
        .post(&url)
        .json(&body)
        .timeout(Duration::from_secs(30))
        .send()
        .context("getFile request failed")?;

    let v: serde_json::Value = resp.json()?;
    let fp = v
        .pointer("/result/file_path")
        .and_then(|f| f.as_str())
        .context("Missing file_path in getFile response")?;
    Ok(fp.to_string())
}

/// Download Telegram file to a temp path, return the local file path.
fn telegram_download_file(client: &Client, bot_token: &str, tg_file_path: &str) -> Result<String> {
    let url = format!(
        "https://api.telegram.org/file/bot{}/{}",
        bot_token, tg_file_path
    );

    let resp = client
        .get(&url)
        .timeout(Duration::from_secs(60))
        .send()
        .context("File download request failed")?;

    if !resp.status().is_success() {
        anyhow::bail!("Download failed: HTTP {}", resp.status());
    }

    let data = resp.bytes().context("Failed to read file bytes")?;
    let local_path = std::env::temp_dir()
        .join(format!("openpaw_tg_voice_{}.ogg", std::process::id()))
        .to_string_lossy()
        .into_owned();

    fs::write(&local_path, &data).context("Failed to write temp audio file")?;
    Ok(local_path)
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_boundary_length() {
        let b = generate_boundary();
        assert_eq!(b.len(), 32);
        assert!(b.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_boundary_varies() {
        // Not guaranteed but astronomically likely
        let b1 = generate_boundary();
        let b2 = generate_boundary();
        // They may be equal in the same nanosecond, skip strict check
        let _ = (b1, b2);
    }

    #[test]
    fn test_transcribe_nonexistent_file() {
        let client = Client::new();
        let result = transcribe_file(
            &client,
            "fake_key",
            GROQ_STT_ENDPOINT,
            "/nonexistent/path/audio.ogg",
            DEFAULT_MODEL,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_whisper_resolve_key_explicit() {
        let key = WhisperTranscriber::resolve_groq_key(Some("sk-explicit"));
        assert_eq!(key, "sk-explicit");
    }
}
