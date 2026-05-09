use anyhow::Result;
use std::collections::HashMap;
use std::fs;
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

    if let Some(parent) = path.parent()
        && let Some(parent_name) = parent.file_name()
        && parent_name == "_generic"
    {
        return None;
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
                if let Some(board) = &chunk.board
                    && boards.contains(&board.as_str())
                {
                    score += 2.0;
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

// ─────────────────────────────────────────────────────────────
// General-purpose workspace knowledge base (lightweight TF-IDF)
// ─────────────────────────────────────────────────────────────

const CHUNK_SIZE: usize = 600; // chars per chunk
const CHUNK_OVERLAP: usize = 100; // overlap between adjacent chunks
const MAX_INDEX_FILE_BYTES: u64 = 512 * 1024; // 512 KB per file
const INDEXED_EXTENSIONS: &[&str] = &["md", "txt", "rst", "org", "log"];
// Directories to skip when walking the workspace
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "__pycache__",
    "skills",
];

/// A single indexed chunk.
#[derive(Debug, Clone)]
pub struct WorkspaceChunk {
    pub source: String, // relative path from workspace root
    pub content: String,
    /// Pre-tokenised lowercase terms for fast scoring.
    terms: Vec<String>,
}

/// Lightweight workspace knowledge base. Zero external dependencies.
#[derive(Debug, Default)]
pub struct WorkspaceRag {
    chunks: Vec<WorkspaceChunk>,
}

impl WorkspaceRag {
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk the workspace and index text files. Call once at startup or on demand.
    pub fn index_workspace(&mut self, workspace_dir: &Path) {
        self.chunks.clear();
        self.walk_dir(workspace_dir, workspace_dir);
    }

    fn walk_dir(&mut self, root: &Path, dir: &Path) {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if path.is_dir() {
                if !SKIP_DIRS.contains(&name) {
                    self.walk_dir(root, &path);
                }
                continue;
            }

            // Only index allowed extensions
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !INDEXED_EXTENSIONS.contains(&ext.as_str()) {
                continue;
            }

            // Skip very large files
            if let Ok(meta) = fs::metadata(&path) {
                if meta.len() > MAX_INDEX_FILE_BYTES {
                    continue;
                }
            }

            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let rel = path
                .strip_prefix(root)
                .ok()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| name.to_string());

            self.add_chunks(&rel, &content);
        }
    }

    fn add_chunks(&mut self, source: &str, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        let total = chars.len();
        if total == 0 {
            return;
        }

        let mut start = 0;
        while start < total {
            let end = (start + CHUNK_SIZE).min(total);
            let chunk_text: String = chars[start..end].iter().collect();
            let terms = tokenise(&chunk_text);
            if !terms.is_empty() {
                self.chunks.push(WorkspaceChunk {
                    source: source.to_string(),
                    content: chunk_text,
                    terms,
                });
            }
            if end == total {
                break;
            }
            start += CHUNK_SIZE - CHUNK_OVERLAP;
        }
    }

    /// Return up to `limit` relevant chunks for the query, ranked by TF-IDF-like score.
    pub fn retrieve<'a>(&'a self, query: &str, limit: usize) -> Vec<&'a WorkspaceChunk> {
        if self.chunks.is_empty() || limit == 0 {
            return Vec::new();
        }

        let query_terms = tokenise(query);
        if query_terms.is_empty() {
            return Vec::new();
        }

        let n_docs = self.chunks.len() as f32;

        // Compute IDF for each query term
        let idf: HashMap<&str, f32> = query_terms
            .iter()
            .map(|term| {
                let df = self
                    .chunks
                    .iter()
                    .filter(|c| c.terms.iter().any(|t| t == term))
                    .count() as f32;
                let idf = if df > 0.0 {
                    (1.0 + n_docs / df).ln()
                } else {
                    0.0
                };
                (term.as_str(), idf)
            })
            .collect();

        let mut scored: Vec<(&WorkspaceChunk, f32)> = self
            .chunks
            .iter()
            .filter_map(|chunk| {
                let n_terms = chunk.terms.len() as f32;
                if n_terms == 0.0 {
                    return None;
                }
                let score: f32 = query_terms
                    .iter()
                    .map(|qt| {
                        let tf = chunk.terms.iter().filter(|t| *t == qt).count() as f32 / n_terms;
                        let idf_val = idf.get(qt.as_str()).copied().unwrap_or(0.0);
                        tf * idf_val
                    })
                    .sum();
                if score > 0.0 {
                    Some((chunk, score))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(limit).map(|(c, _)| c).collect()
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

/// Tokenise text into lowercase, alpha-only terms ≥ 3 chars.
/// Stop-words are filtered to reduce noise.
fn tokenise(text: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "and", "for", "are", "was", "were", "this", "that", "with", "have", "has", "had",
        "not", "but", "from", "you", "your", "its", "will", "can", "all", "one", "our", "out",
        "use", "used", "also", "than", "then", "into", "more", "their", "they", "been", "being",
    ];
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_lowercase())
        .filter(|t| !STOP.contains(&t.as_str()))
        .collect()
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
