use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs;

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
    5 * 1024 * 1024
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

pub struct ParseResult {
    pub cleaned_text: String,
    pub refs: Vec<String>,
}

pub struct ImageData {
    pub data: Vec<u8>,
    pub mime_type: String,
}

pub fn parse_image_markers(content: &str) -> ParseResult {
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
                    let kind_str = &marker[..colon_pos];
                    let target = marker[colon_pos + 1..].trim();

                    if !target.is_empty() && is_image_kind(kind_str) {
                        refs.push(target.to_string());
                        cursor = close_pos + 1;
                        continue;
                    }
                }

                // Not a valid [IMAGE:] marker
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

fn is_image_kind(kind: &str) -> bool {
    let k = kind.to_lowercase();
    k == "image" || k == "photo" || k == "img"
}

pub fn detect_mime_type(data: &[u8]) -> Option<&'static str> {
    if data.len() < 4 {
        return None;
    }

    // PNG: 89 50 4E 47
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some("image/png");
    }

    // JPEG: FF D8 FF
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }

    // GIF: GIF8
    if data.starts_with(b"GIF8") {
        return Some("image/gif");
    }

    // BMP: BM
    if data.starts_with(b"BM") {
        return Some("image/bmp");
    }

    // WebP: RIFF....WEBP
    if data.len() >= 12
        && data.starts_with(b"RIFF")
        && &data[8..12] == b"WEBP"
    {
        return Some("image/webp");
    }

    None
}

pub fn read_local_image(path: &str, config: &MultimodalConfig) -> Result<ImageData> {
    let resolved = fs::canonicalize(path)?;

    // Verify path is in allowed_dirs
    if config.allowed_dirs.is_empty() {
        return Err(anyhow!("Local file reads not allowed (allowed_dirs empty)"));
    }

    let mut allowed = false;
    for dir in &config.allowed_dirs {
        let allowed_path = fs::canonicalize(dir)?;
        if resolved.starts_with(&allowed_path) {
            allowed = true;
            break;
        }
    }

    if !allowed {
        return Err(anyhow!("Path not allowed: {:?}", resolved));
    }

    let metadata = fs::metadata(&resolved)?;
    if metadata.len() > config.max_image_size_bytes {
        return Err(anyhow!(
            "Image too large: {} > {}",
            metadata.len(),
            config.max_image_size_bytes
        ));
    }

    let data = fs::read(&resolved)?;
    let mime_type = detect_mime_type(&data).ok_or_else(|| anyhow!("Unknown image format"))?;

    Ok(ImageData {
        data,
        mime_type: mime_type.to_string(),
    })
}
