/// Minimal SSE line parser for streaming LLM responses.
///
/// Wraps a `reqwest::blocking::Response` body and yields `data:` field values
/// one at a time. Used by Anthropic provider.
use std::io::{BufRead, BufReader, Read};

pub struct SseReader<R: Read> {
    reader: BufReader<R>,
}

impl<R: Read> SseReader<R> {
    pub fn new(body: R) -> Self {
        Self {
            reader: BufReader::new(body),
        }
    }

    /// Return the next non-empty `data:` line payload, or `None` on EOF.
    /// Lines containing only `[DONE]` are skipped; callers should check.
    pub fn next_data(&mut self) -> Option<String> {
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None, // EOF
                Ok(_) => {}
                Err(_) => return None,
            }

            let trimmed = line.trim_end_matches(['\n', '\r']);

            if let Some(rest) = trimmed.strip_prefix("data:") {
                let payload = rest.trim();
                if payload.is_empty() {
                    continue;
                }
                return Some(payload.to_string());
            }
            // Skip event:, id:, comment lines, blank lines
        }
    }
}

/// Convenience — collect all `data:` payloads from an SSE response into a Vec.
/// Stops at `[DONE]`.
pub fn collect_data_lines<R: Read>(body: R) -> Vec<String> {
    let mut reader = SseReader::new(body);
    let mut out = Vec::new();
    while let Some(line) = reader.next_data() {
        if line == "[DONE]" {
            break;
        }
        out.push(line);
    }
    out
}
