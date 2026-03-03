use super::{Tool, ToolResult};
use anyhow::Result;
use serde_json::Value;
use std::fs::File;
use std::io::Read;
use std::path::Path;

const MAX_IMAGE_BYTES: u64 = 5_242_880;

pub struct ImageInfoTool {}

impl Tool for ImageInfoTool {
    fn name(&self) -> &str {
        "image_info"
    }

    fn description(&self) -> &str {
        "Read image file metadata (format, dimensions, size)."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Path to the image file"},"include_base64":{"type":"boolean","description":"Include base64-encoded data (default: false)"}},"required":["path"]}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        let path_str = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolResult::fail("Missing 'path' parameter")),
        };

        let path = Path::new(path_str);

        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(e) => {
                return Ok(ToolResult::fail(format!(
                    "File not found: {} ({})",
                    path_str, e
                )));
            }
        };

        let metadata = match file.metadata() {
            Ok(m) => m,
            Err(e) => return Ok(ToolResult::fail(format!("Failed to read metadata: {}", e))),
        };

        let size = metadata.len();
        if size > MAX_IMAGE_BYTES {
            return Ok(ToolResult::fail(format!(
                "Image too large: {} bytes (max {} bytes)",
                size, MAX_IMAGE_BYTES
            )));
        }

        let mut header = [0u8; 128];
        let bytes_read = file.read(&mut header).unwrap_or(0);
        let bytes = &header[0..bytes_read];

        let format = detect_format(bytes);
        let dimensions = extract_dimensions(bytes, format);

        let mut output = format!(
            "File: {}\nFormat: {}\nSize: {} bytes",
            path_str, format, size
        );

        if let Some((w, h)) = dimensions {
            output.push_str(&format!("\nDimensions: {}x{}", w, h));
        }

        Ok(ToolResult::ok(output))
    }
}

pub fn detect_format(bytes: &[u8]) -> &'static str {
    if bytes.len() < 4 {
        return "unknown";
    }
    if bytes[0] == 0x89 && bytes[1] == b'P' && bytes[2] == b'N' && bytes[3] == b'G' {
        return "png";
    }
    if bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return "jpeg";
    }
    if bytes[0] == b'G' && bytes[1] == b'I' && bytes[2] == b'F' && bytes[3] == b'8' {
        return "gif";
    }
    if bytes[0] == b'R' && bytes[1] == b'I' && bytes[2] == b'F' && bytes[3] == b'F' {
        if bytes.len() >= 12
            && bytes[8] == b'W'
            && bytes[9] == b'E'
            && bytes[10] == b'B'
            && bytes[11] == b'P'
        {
            return "webp";
        }
    }
    if bytes[0] == b'B' && bytes[1] == b'M' {
        return "bmp";
    }
    "unknown"
}

pub fn extract_dimensions(bytes: &[u8], format: &str) -> Option<(u32, u32)> {
    if format == "png" && bytes.len() >= 24 {
        let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        return Some((w, h));
    }
    if format == "gif" && bytes.len() >= 10 {
        let w = u16::from_le_bytes(bytes[6..8].try_into().unwrap()) as u32;
        let h = u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as u32;
        return Some((w, h));
    }
    if format == "bmp" && bytes.len() >= 26 {
        let w = u32::from_le_bytes(bytes[18..22].try_into().unwrap());
        let h_raw = i32::from_le_bytes(bytes[22..26].try_into().unwrap());
        let h = h_raw.abs() as u32;
        return Some((w, h));
    }
    if format == "jpeg" {
        return jpeg_dimensions(bytes);
    }
    None
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2; // skip SOI marker
    while i + 1 < bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        i += 2;

        if marker >= 0xC0 && marker <= 0xC3 {
            if i + 7 <= bytes.len() {
                let h = u16::from_be_bytes(bytes[i + 3..i + 5].try_into().unwrap()) as u32;
                let w = u16::from_be_bytes(bytes[i + 5..i + 7].try_into().unwrap()) as u32;
                return Some((w, h));
            }
            return None;
        }

        if i + 1 < bytes.len() {
            let seg_len = u16::from_be_bytes(bytes[i..i + 2].try_into().unwrap()) as usize;
            if seg_len < 2 {
                return None;
            }
            i += seg_len;
        } else {
            return None;
        }
    }
    None
}
