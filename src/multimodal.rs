use anyhow::{anyhow, Result, Context};
use serde::{Deserialize, Serialize};
use std::fs;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultimodalConfig {
    #[serde(default = "default_max_images")]
    pub max_images: u32,
    #[serde(default = "default_max_image_size_bytes")]
    pub max_image_size_bytes: u64,
    #[serde(default)]
    pub allow_remote_fetch: bool,
    #[serde(default)]
    pub allowed_dirs: Vec<String>,
}

fn default_max_images() -> u32 {
    4
}

fn default_max_image_size_bytes() -> u64 {
    20 * 1024 * 1024 // Increased to 20MB for video/audio
}

impl Default for MultimodalConfig {
    fn default() -> Self {
        Self {
            max_images: default_max_images(),
            max_image_size_bytes: default_max_image_size_bytes(),
            allow_remote_fetch: false,
            allowed_dirs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultimodalKind {
    Image,
    Audio,
    Video,
    File,
}

pub struct MultimodalRef {
    pub kind: MultimodalKind,
    pub path: String,
}

pub struct ParseResult {
    pub cleaned_text: String,
    pub refs: Vec<MultimodalRef>,
}

pub struct MultimodalData {
    pub data: Vec<u8>,
    pub mime_type: String,
    pub kind: MultimodalKind,
}

pub fn parse_multimodal_markers(content: &str) -> ParseResult {
    let mut cleaned_text = String::with_capacity(content.len());
    let mut refs = Vec::new();
    let mut cursor = 0;

    while cursor < content.len() {
        if let Some(open_pos) = content[cursor..].find('[') {
            let open_pos = cursor + open_pos;
            cleaned_text.push_str(&content[cursor..open_pos]);

            if let Some(close_pos) = content[open_pos..].find(']') {
                let close_pos = open_pos + close_pos;
                let marker = &content[open_pos + 1..close_pos];

                if let Some(colon_pos) = marker.find(':') {
                    let kind_str = marker[..colon_pos].to_lowercase();
                    let target = marker[colon_pos + 1..].trim();

                    if !target.is_empty() {
                        let kind = match kind_str.as_str() {
                            "image" | "photo" | "img" => Some(MultimodalKind::Image),
                            "audio" | "voice" | "sound" => Some(MultimodalKind::Audio),
                            "video" | "vid" | "movie" => Some(MultimodalKind::Video),
                            "file" | "doc" | "document" => Some(MultimodalKind::File),
                            _ => None,
                        };

                        if let Some(k) = kind {
                            refs.push(MultimodalRef {
                                kind: k,
                                path: target.to_string(),
                            });
                            cursor = close_pos + 1;
                            continue;
                        }
                    }
                }

                cleaned_text.push_str(&content[open_pos..=close_pos]);
                cursor = close_pos + 1;
            } else {
                cleaned_text.push_str(&content[open_pos..]);
                break;
            }
        } else {
            cleaned_text.push_str(&content[cursor..]);
            break;
        }
    }

    ParseResult {
        cleaned_text: cleaned_text.trim().to_string(),
        refs,
    }
}

pub fn detect_mime_type(data: &[u8], path: &str) -> String {
    if data.len() >= 4 {
        if data.starts_with(&[0x89, b'P', b'N', b'G']) { return "image/png".to_string(); }
        if data.starts_with(&[0xFF, 0xD8, 0xFF]) { return "image/jpeg".to_string(); }
        if data.starts_with(b"GIF8") { return "image/gif".to_string(); }
        if data.starts_with(b"BM") { return "image/bmp".to_string(); }
        if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" { return "image/webp".to_string(); }
    }

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "mp3" => "audio/mpeg".to_string(),
        "wav" => "audio/wav".to_string(),
        "ogg" => "audio/ogg".to_string(),
        "m4a" => "audio/mp4".to_string(),
        "mp4" => "video/mp4".to_string(),
        "mpeg" | "mpg" => "video/mpeg".to_string(),
        "mov" => "video/quicktime".to_string(),
        "webm" => "video/webm".to_string(),
        "pdf" => "application/pdf".to_string(),
        "txt" => "text/plain".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

pub fn is_gemini_cli_available() -> bool {
    Command::new("gemini")
        .arg("--version")
        .output()
        .is_ok()
}

pub fn process_with_gemini_cli(prompt: &str, files: &[String]) -> Result<String> {
    let mut cmd = Command::new("gemini");
    cmd.arg("ask");
    
    // Construct the prompt with file references
    let full_prompt = prompt.to_string();
    for file in files {
        // @google/gemini-cli uses @path to reference files
        cmd.arg(format!("@{}", file));
    }
    
    cmd.arg(full_prompt);

    let output = cmd.output().context("Failed to execute gemini cli")?;
    if !output.status.success() {
        return Err(anyhow!("Gemini CLI error: {}", String::from_utf8_lossy(&output.stderr)));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn read_local_file(path: &str, config: &MultimodalConfig) -> Result<MultimodalData> {
    let resolved = fs::canonicalize(path)?;

    // Verify path is in allowed_dirs
    if config.allowed_dirs.is_empty() {
        return Err(anyhow!("Local file reads not allowed (allowed_dirs empty)"));
    }

    let mut allowed = false;
    for dir in &config.allowed_dirs {
        if let Ok(allowed_path) = fs::canonicalize(dir) {
            if resolved.starts_with(&allowed_path) {
                allowed = true;
                break;
            }
        }
    }

    if !allowed {
        return Err(anyhow!("Path not allowed: {:?}", resolved));
    }

    let metadata = fs::metadata(&resolved)?;
    if metadata.len() > config.max_image_size_bytes {
        return Err(anyhow!(
            "File too large: {} > {}",
            metadata.len(),
            config.max_image_size_bytes
        ));
    }

    let data = fs::read(&resolved)?;
    let mime_type = detect_mime_type(&data, path);
    
    let kind = if mime_type.starts_with("image/") {
        MultimodalKind::Image
    } else if mime_type.starts_with("audio/") {
        MultimodalKind::Audio
    } else if mime_type.starts_with("video/") {
        MultimodalKind::Video
    } else {
        MultimodalKind::File
    };

    Ok(MultimodalData {
        data,
        mime_type,
        kind,
    })
}
