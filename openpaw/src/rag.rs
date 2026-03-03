use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

/// A chunk of datasheet content with board metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasheetChunk {
    /// Board this chunk applies to (e.g. "nucleo-f401re"), or null for generic.
    pub board: Option<String>,
    /// Source file path.
    pub source: String,
    /// Chunk content.
    pub content: String,
}

/// Pin alias: human-readable name to pin number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinAlias {
    pub alias: String,
    pub pin: u32,
}

/// Parse pin aliases from markdown content.
/// Looks for a `## Pin Aliases` section with `alias: pin` lines
/// or markdown table `| alias | pin |` rows.
pub fn parse_pin_aliases(content: &str) -> Result<Vec<PinAlias>> {
    let lower = content.to_lowercase();
    let markers = ["## pin aliases", "## pin alias", "## pins"];

    let mut section_start = None;
    for marker in markers {
        if let Some(pos) = lower.find(marker) {
            section_start = Some(pos + marker.len());
            break;
        }
    }

    let start = match section_start {
        Some(s) => s,
        None => return Ok(Vec::new()),
    };

    let rest = &content[start..];
    let end = rest
        .find("\n## ")
        .map(|i| start + i)
        .unwrap_or(content.len());
    let section = &content[start..end];

    let mut aliases = Vec::new();
    for raw_line in section.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with('|') {
            if let Some(alias) = parse_table_row(line) {
                aliases.push(alias);
            }
            continue;
        }

        if let Some(alias) = parse_key_value(line) {
            aliases.push(alias);
        }
    }

    Ok(aliases)
}

fn parse_table_row(line: &str) -> Option<PinAlias> {
    let mut parts = line.split('|');
    parts.next()?; // Skip leading empty part

    let alias_raw = parts.next()?;
    let pin_raw = parts.next()?;

    let alias = alias_raw.trim();
    let pin_str = pin_raw.trim();

    if alias == "alias" || alias == "pin" {
        return None;
    }
    if alias.contains("---") || pin_str.contains("---") {
        return None;
    }
    if pin_str == "pin" {
        return None;
    }
    if alias.is_empty() {
        return None;
    }

    let pin = pin_str.parse::<u32>().ok()?;
    Some(PinAlias {
        alias: alias.to_string(),
        pin,
    })
}

fn parse_key_value(line: &str) -> Option<PinAlias> {
    let sep_pos = line.find(':').or_else(|| line.find('='))?;
    let alias = line[..sep_pos].trim();
    let pin_str = line[sep_pos + 1..].trim();

    if alias.is_empty() {
        return None;
    }
    let pin = pin_str.parse::<u32>().ok()?;
    Some(PinAlias {
        alias: alias.to_string(),
        pin,
    })
}

/// Infer board tag from a file path. "nucleo-f401re.md" -> "nucleo-f401re".
/// Returns null for "generic" or "_generic" paths.
pub fn infer_board_from_path(path_str: &str) -> Option<String> {
    let path = Path::new(path_str);
    let stem = path.file_stem()?.to_str()?;

    if stem.is_empty() {
        return None;
    }
    if stem == "generic" || stem.starts_with("generic_") {
        return None;
    }

    if let Some(parent) = path.parent() {
        if let Some(parent_name) = parent.file_name() {
            if parent_name == "_generic" {
                return None;
            }
        }
    }

    Some(stem.to_string())
}

/// Hardware RAG index -- stores datasheet chunks and pin aliases.
#[derive(Debug)]
pub struct HardwareRag {
    pub chunks: Vec<DatasheetChunk>,
    pub pin_aliases: HashMap<String, Vec<PinAlias>>,
}

impl Default for HardwareRag {
    fn default() -> Self {
        Self::new()
    }
}

impl HardwareRag {
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            pin_aliases: HashMap::new(),
        }
    }

    /// Number of indexed chunks.
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// True if no chunks are indexed.
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Get pin aliases for a board.
    pub fn pin_aliases_for_board(&self, board: &str) -> Option<&[PinAlias]> {
        self.pin_aliases.get(board).map(|v| v.as_slice())
    }

    /// Retrieve chunks relevant to the query and boards.
    /// Uses keyword matching and board filter.
    pub fn retrieve(&self, query: &str, boards: &[&str], limit: usize) -> Vec<&DatasheetChunk> {
        if self.chunks.is_empty() || limit == 0 {
            return Vec::new();
        }

        let query_lower = query.to_lowercase();
        let terms: Vec<&str> = query_lower
            .split_whitespace()
            .filter(|t| t.len() > 2)
            .collect();

        if terms.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<(&DatasheetChunk, f32)> = Vec::new();
        for chunk in &self.chunks {
            let content_lower = chunk.content.to_lowercase();
            let mut score = 0.0;

            for term in &terms {
                if content_lower.contains(term) {
                    score += 1.0;
                }
            }

            if score > 0.0 {
                if let Some(board) = &chunk.board {
                    if boards.contains(&board.as_str()) {
                        score += 2.0;
                    }
                }
                scored.push((chunk, score));
            }
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .take(limit)
            .map(|(chunk, _)| chunk)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pin_aliases_key_value() {
        let md = "## Pin Aliases\n\
                 red_led: 13\n\
                 builtin_led: 13\n\
                 user_led: 5";
        let aliases = parse_pin_aliases(md).unwrap();
        assert_eq!(aliases.len(), 3);
        assert_eq!(aliases[0].alias, "red_led");
        assert_eq!(aliases[0].pin, 13);
        assert_eq!(aliases[1].alias, "builtin_led");
        assert_eq!(aliases[1].pin, 13);
        assert_eq!(aliases[2].alias, "user_led");
        assert_eq!(aliases[2].pin, 5);
    }

    #[test]
    fn test_parse_pin_aliases_table_format() {
        let md = "## Pin Aliases\n\
                 | alias | pin |\n\
                 |-------|-----|\n\
                 | red_led | 13 |\n\
                 | builtin_led | 13 |";
        let aliases = parse_pin_aliases(md).unwrap();
        assert_eq!(aliases.len(), 2);
        assert_eq!(aliases[0].alias, "red_led");
        assert_eq!(aliases[0].pin, 13);
    }

    #[test]
    fn test_parse_pin_aliases_empty_when_no_section() {
        let aliases = parse_pin_aliases("No aliases here").unwrap();
        assert_eq!(aliases.len(), 0);
    }

    #[test]
    fn test_parse_pin_aliases_equals_separator() {
        let md = "## Pin Aliases\n\
                 led = 13\n\
                 button = 2";
        let aliases = parse_pin_aliases(md).unwrap();
        assert_eq!(aliases.len(), 2);
        assert_eq!(aliases[0].alias, "led");
        assert_eq!(aliases[0].pin, 13);
    }

    #[test]
    fn test_parse_pin_aliases_ignores_non_numeric() {
        let md = "## Pin Aliases\n\
                 name: test\n\
                 led: 13";
        let aliases = parse_pin_aliases(md).unwrap();
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].pin, 13);
    }

    #[test]
    fn test_parse_pin_aliases_stops_at_next_heading() {
        let md = "## Pin Aliases\n\
                 led: 13\n\
                 ## GPIO\n\
                 something: 99";
        let aliases = parse_pin_aliases(md).unwrap();
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].pin, 13);
    }

    #[test]
    fn test_infer_board_from_path() {
        assert_eq!(
            infer_board_from_path("datasheets/nucleo-f401re.md").unwrap(),
            "nucleo-f401re"
        );
        assert!(infer_board_from_path("datasheets/generic.md").is_none());
        assert!(infer_board_from_path("datasheets/generic_notes.md").is_none());
        assert!(infer_board_from_path("datasheets/_generic/notes.md").is_none());
        assert_eq!(
            infer_board_from_path("ds/rpi-gpio.txt").unwrap(),
            "rpi-gpio"
        );
    }

    #[test]
    fn test_hardwarerag_init() {
        let rag = HardwareRag::new();
        assert!(rag.is_empty());
        assert_eq!(rag.len(), 0);
    }

    #[test]
    fn test_hardwarerag_retrieve_empty() {
        let rag = HardwareRag::new();
        let results = rag.retrieve("led", &["test-board"], 5);
        assert!(results.is_empty());
    }
}
