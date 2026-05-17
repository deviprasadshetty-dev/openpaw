use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
    // Keep inline provider payloads comfortably below the OpenAI-compatible
    // 8MB request cap after base64 expansion and JSON overhead. Larger local
    // media should be analyzed path-first through the vision tool/Gemini CLI.
    4 * 1024 * 1024
}

const INLINE_PROVIDER_MEDIA_LIMIT_BYTES: u64 = 4 * 1024 * 1024;

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
        if data.starts_with(&[0x89, b'P', b'N', b'G']) {
            return "image/png".to_string();
        }
        if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return "image/jpeg".to_string();
        }
        if data.starts_with(b"GIF8") {
            return "image/gif".to_string();
        }
        if data.starts_with(b"BM") {
            return "image/bmp".to_string();
        }
        if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
            return "image/webp".to_string();
        }
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
    gemini_command_candidates().iter().any(|cmd| {
        Command::new(cmd)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

pub fn process_with_gemini_cli(prompt: &str, files: &[String]) -> Result<String> {
    let prompt = current_request_only(prompt);
    let resolved_files = resolve_gemini_files(files)?;
    let (cwd, include_dirs) = gemini_workspace_for_files(&resolved_files)?;

    let mut full_prompt = String::new();
    full_prompt.push_str(
        "Analyze the referenced local file(s) for the current user request only. \
         Ignore any historical memory context if it appears in the prompt.\n\n",
    );
    for abs in &resolved_files {
        full_prompt.push_str(&format!(
            "Read and analyze the file at path: {}\n",
            abs.display()
        ));
    }
    full_prompt.push_str("\nCurrent request:\n");
    full_prompt.push_str(&prompt);

    let output = run_gemini_cli(&full_prompt, &cwd, &include_dirs)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Gemini CLI error: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err(anyhow!("Gemini CLI returned empty output"));
    }

    Ok(stdout)
}

fn gemini_command_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    {
        candidates.push("gemini.cmd".to_string());
        candidates.push("gemini.exe".to_string());
    }
    candidates.push("gemini".to_string());
    candidates
}

fn resolve_gemini_files(files: &[String]) -> Result<Vec<PathBuf>> {
    if files.is_empty() {
        return Err(anyhow!("No files provided for Gemini CLI"));
    }

    files
        .iter()
        .map(|file| {
            std::fs::canonicalize(file)
                .map_err(|e| anyhow!("Failed to resolve Gemini input file '{}': {}", file, e))
        })
        .collect()
}

fn gemini_workspace_for_files(files: &[PathBuf]) -> Result<(PathBuf, Vec<PathBuf>)> {
    let first = files
        .first()
        .ok_or_else(|| anyhow!("No files provided for Gemini CLI"))?;
    let cwd = first
        .parent()
        .filter(|p| p.is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let mut include_dirs = Vec::new();
    for file in files {
        if let Some(parent) = file.parent()
            && parent != cwd
            && !include_dirs.iter().any(|dir: &PathBuf| dir == parent)
        {
            include_dirs.push(parent.to_path_buf());
        }
    }

    Ok((cwd, include_dirs))
}

fn run_gemini_cli(prompt: &str, cwd: &Path, include_dirs: &[PathBuf]) -> Result<Output> {
    let mut failures = Vec::new();
    for cmd in gemini_command_candidates() {
        let mut command = Command::new(&cmd);
        command
            .current_dir(cwd)
            .arg("--skip-trust")
            .arg("--approval-mode")
            .arg("yolo")
            .arg("--output-format")
            .arg("text");

        for dir in include_dirs {
            command.arg("--include-directories").arg(dir);
        }

        match command.arg("-p").arg(prompt).output() {
            Ok(output) => return Ok(output),
            Err(e) => failures.push(format!("{}: {}", cmd, e)),
        }
    }

    Err(anyhow!(
        "Failed to execute Gemini CLI. Tried: {}",
        failures.join("; ")
    ))
}

pub fn current_request_only(content: &str) -> String {
    if let Some(inner) = extract_tag(content, "current_request") {
        return inner.trim().to_string();
    }

    let without_underscore = remove_tagged_blocks(content, "memory_context");
    remove_tagged_blocks(&without_underscore, "memory-context")
        .trim()
        .to_string()
}

fn extract_tag<'a>(content: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = content.find(&open)? + open.len();
    let end = content[start..].find(&close)? + start;
    Some(&content[start..end])
}

fn remove_tagged_blocks(content: &str, tag: &str) -> String {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let mut output = String::with_capacity(content.len());
    let mut cursor = 0;

    while let Some(rel_start) = content[cursor..].find(&open) {
        let start = cursor + rel_start;
        output.push_str(&content[cursor..start]);

        let after_open = start + open.len();
        if let Some(rel_end) = content[after_open..].find(&close) {
            cursor = after_open + rel_end + close.len();
        } else {
            cursor = after_open;
            break;
        }
    }

    output.push_str(&content[cursor..]);
    output
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
    let max_inline_bytes = config
        .max_image_size_bytes
        .min(INLINE_PROVIDER_MEDIA_LIMIT_BYTES);
    if metadata.len() > max_inline_bytes {
        return Err(anyhow!(
            "File too large for inline multimodal payload: {} > {} bytes. Use the vision tool for larger local media.",
            metadata.len(),
            max_inline_bytes
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_request_only_extracts_current_request() {
        let content = "<memory_context>\nold task\n</memory_context>\n\n<current_request>\ninspect this screenshot\n</current_request>";

        assert_eq!(current_request_only(content), "inspect this screenshot");
    }

    #[test]
    fn current_request_only_removes_memory_blocks_without_request_tag() {
        let content =
            "<memory-context>\npreferences and history\n</memory-context>\n\nWhat is in this file?";

        assert_eq!(current_request_only(content), "What is in this file?");
    }

    #[test]
    fn parse_multimodal_markers_detects_telegram_image_marker_with_caption() {
        let path = r"C:\Users\Deviprasad Shetty\AppData\Local\Temp\openpaw-tg-inbound\photo.jpg";
        let content = format!("[IMAGE:{}]\nCaption: See", path);
        let parsed = parse_multimodal_markers(&content);

        assert_eq!(parsed.refs.len(), 1);
        assert_eq!(parsed.refs[0].kind, MultimodalKind::Image);
        assert_eq!(parsed.refs[0].path, path);
        assert_eq!(parsed.cleaned_text, "Caption: See");
    }

    #[test]
    fn read_local_file_rejects_files_above_inline_payload_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("large.bin");
        fs::write(
            &path,
            vec![0u8; (INLINE_PROVIDER_MEDIA_LIMIT_BYTES + 1) as usize],
        )
        .expect("write test file");

        let config = MultimodalConfig {
            max_image_size_bytes: 20 * 1024 * 1024,
            allowed_dirs: vec![dir.path().to_string_lossy().to_string()],
            ..Default::default()
        };

        let err = match read_local_file(&path.to_string_lossy(), &config) {
            Ok(_) => panic!("large inline file should be rejected"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("Use the vision tool"));
    }

    #[test]
    fn gemini_workspace_uses_first_file_parent_and_includes_other_dirs() {
        let first_dir = tempfile::tempdir().expect("first tempdir");
        let second_dir = tempfile::tempdir().expect("second tempdir");
        let first = first_dir.path().join("a.png");
        let second = second_dir.path().join("b.png");
        fs::write(&first, b"a").expect("write first");
        fs::write(&second, b"b").expect("write second");

        let files = resolve_gemini_files(&[
            first.to_string_lossy().to_string(),
            second.to_string_lossy().to_string(),
        ])
        .expect("resolve files");
        let (cwd, include_dirs) = gemini_workspace_for_files(&files).expect("workspace");

        assert_eq!(cwd, std::fs::canonicalize(first_dir.path()).unwrap());
        assert!(include_dirs.contains(&std::fs::canonicalize(second_dir.path()).unwrap()));
    }
}
