use super::{Tool, ToolContext, ToolResult};
use crate::rag::WorkspaceRag;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

pub struct WorkspaceSearchTool {
    pub workspace_dir: String,
}

#[async_trait]
impl Tool for WorkspaceSearchTool {
    fn name(&self) -> &str {
        "workspace_search"
    }

    fn description(&self) -> &str {
        "Search the workspace knowledge base (markdown, text, and doc files) for content relevant to a query. Returns the most relevant excerpts with source file paths."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"query":{"type":"string","description":"Natural language search query"},"limit":{"type":"integer","description":"Maximum results to return (default 5, max 10)"}},"required":["query"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim(),
            _ => return Ok(ToolResult::fail("Missing or empty 'query' parameter")),
        };

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n.min(10) as usize)
            .unwrap_or(5);

        // Index is built fresh per call — lightweight enough for small workspaces.
        // For larger workspaces this stays fast because we cap file size and skip
        // build dirs. A persistent index can be added later without API changes.
        let mut rag = WorkspaceRag::new();
        rag.index_workspace(Path::new(&self.workspace_dir));

        if rag.is_empty() {
            return Ok(ToolResult::ok(
                "No indexable documents found in workspace (looking for .md, .txt, .rst files).",
            ));
        }

        let results = rag.retrieve(query, limit);
        if results.is_empty() {
            return Ok(ToolResult::ok(format!(
                "No relevant content found for: \"{}\"",
                query
            )));
        }

        let mut out = format!(
            "Found {} relevant excerpt(s) for \"{}\":\n\n",
            results.len(),
            query
        );
        for (i, chunk) in results.iter().enumerate() {
            out.push_str(&format!(
                "--- [{}/{}] {} ---\n{}\n\n",
                i + 1,
                results.len(),
                chunk.source,
                chunk.content.trim()
            ));
        }

        Ok(ToolResult::ok(out))
    }
}
