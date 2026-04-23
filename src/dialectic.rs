use crate::providers::{ChatMessage, ChatRequest};
use std::path::Path;
use std::sync::Arc;

const DIALECTIC_FILE: &str = "DIALECTIC.md";

/// Analyze a completed session and update the dialectic user model.
/// This is a lightweight post-processing step that extracts meta-context
/// like "User is impatient with UI tasks but patient with backend issues."
pub async fn analyze_session(
    provider: Arc<dyn crate::providers::Provider>,
    model_name: &str,
    history: &[ChatMessage],
    workspace_dir: &str,
) {
    if history.len() < 4 {
        return; // Not enough conversation to analyze
    }

    let dialectic_path = Path::new(workspace_dir).join(DIALECTIC_FILE);

    // Build a summary of the conversation for analysis
    let mut summary = String::new();
    for msg in history.iter().rev().take(20).rev() {
        let prefix = match msg.role.as_str() {
            "user" => "User",
            "assistant" => "Assistant",
            _ => continue,
        };
        let content = msg.content.trim();
        if content.len() > 300 {
            summary.push_str(&format!("{}: {}...\n", prefix, &content[..300]));
        } else {
            summary.push_str(&format!("{}: {}\n", prefix, content));
        }
    }

    let existing = match tokio::fs::read_to_string(&dialectic_path).await {
        Ok(c) => c,
        Err(_) => String::new(),
    };

    let prompt = format!(
        "You are a user-modeling analyst. Given the recent conversation and the existing user profile, \
         extract or update concise meta-context about the user. Focus on: communication style, patience levels, \
         domain preferences, frustration triggers, and work habits.\n\n\
         Existing profile:\n{}\n\n\
         Recent conversation:\n{}\n\n\
         Respond with ONLY the updated profile text (max 800 chars). Be concise. \
         If nothing new is worth adding, respond with the existing profile unchanged.",
        if existing.is_empty() { "(empty)" } else { &existing },
        summary
    );

    let request = ChatRequest {
        messages: &[ChatMessage::user(prompt)],
        model: model_name,
        temperature: 0.2,
        max_tokens: Some(600),
        tools: None,
        timeout_secs: 30,
        reasoning_effort: None,
    };

    let response = match provider.chat(&request) {
        Ok(r) => r,
        Err(_) => return,
    };

    let content = match response.content {
        Some(c) => c.trim().to_string(),
        None => return,
    };

    if content.len() > 1200 {
        // Truncate if too long
        let truncated = &content[..1200];
        let _ = tokio::fs::write(&dialectic_path, truncated).await;
    } else if !content.is_empty() && content != existing {
        let _ = tokio::fs::write(&dialectic_path, content).await;
    }
}

/// Read the dialectic user model for prompt injection.
pub fn load_dialectic_context(workspace_dir: &str) -> String {
    let path = Path::new(workspace_dir).join(DIALECTIC_FILE);
    match std::fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => content,
        _ => String::new(),
    }
}
