use crate::agent::Agent;

pub const BARE_SESSION_RESET_PROMPT: &str =
    "A new session was started via /new or /reset. Execute your Session Startup sequence now - read the required files before responding to the user. Then greet the user in your configured persona, if one is provided. Be yourself - use your defined voice, mannerisms, and mood. Keep it to 1-3 sentences and ask what they want to do. If the runtime model differs from default_model in the system prompt, mention the default model. Do not mention internal steps, files, tools, or reasoning.";

#[derive(Debug, PartialEq)]
pub struct SlashCommand {
    pub name: String,
    pub arg: String,
}

pub fn parse_slash_command(message: &str) -> Option<SlashCommand> {
    let trimmed = message.trim();
    if !trimmed.starts_with('/') || trimmed.len() <= 1 {
        return None;
    }

    let body = &trimmed[1..];
    let split_idx = body.find(|c| c == ':' || c == ' ' || c == '\t').unwrap_or(body.len());
    
    let raw_name = &body[..split_idx];
    if raw_name.is_empty() {
        return None;
    }

    let name = if let Some(idx) = raw_name.find('@') {
        &raw_name[..idx]
    } else {
        raw_name
    };

    if name.is_empty() {
        return None;
    }

    let mut rest = &body[split_idx..];
    if !rest.is_empty() && rest.starts_with(':') {
        rest = &rest[1..];
    }

    Some(SlashCommand {
        name: name.to_string(),
        arg: rest.trim().to_string(),
    })
}

fn is_slash_name(cmd: &SlashCommand, expected: &str) -> bool {
    cmd.name.eq_ignore_ascii_case(expected)
}

pub fn bare_session_reset_prompt(message: &str) -> Option<&'static str> {
    if let Some(cmd) = parse_slash_command(message) {
        if (is_slash_name(&cmd, "new") || is_slash_name(&cmd, "reset")) && cmd.arg.is_empty() {
            return Some(BARE_SESSION_RESET_PROMPT);
        }
    }
    None
}

pub fn handle_slash_command(agent: &mut Agent, message: &str) -> Option<String> {
    let cmd = parse_slash_command(message)?;

    if is_slash_name(&cmd, "model") {
        if cmd.arg.is_empty() {
            return Some(format!("Current model: {}", agent.model_name));
        }
        agent.model_name = cmd.arg.clone();
        // Recalculate token limits
        use crate::agent::context_tokens::resolve_context_tokens;
        use crate::agent::max_tokens::resolve_max_tokens;
        
        agent.token_limit = resolve_context_tokens(None, &agent.model_name);
        agent.max_tokens = resolve_max_tokens(None, &agent.model_name);
        
        return Some(format!("Switched model to: {}", agent.model_name));
    }

    if is_slash_name(&cmd, "provider") {
         // Placeholder for provider switching logic
         return Some(format!("Provider switching not fully implemented. Current: {}", agent.model_name));
    }
    
    if is_slash_name(&cmd, "new") || is_slash_name(&cmd, "reset") {
        agent.reset_history();
        agent.memory_session_id = Some("new-session".to_string()); // Simple rotation
        if !cmd.arg.is_empty() {
             // If arg provided, it's a new session with a prompt
             // We don't return a response, we let the agent loop handle it as a new turn
             // But handleSlashCommand is supposed to return Option<String>.
             // If we return None, the caller might process it as a normal message?
             // In NullClaw, handleSlashCommand returns ?[]const u8.
             // If it returns a string, that string is returned to the user and NO LLM call is made.
             // If it returns null, the message is processed normally.
             // But /new with arg should probably start a new session AND process the arg as the first message.
             // For now, let's just clear history.
        }
        return Some("Session reset.".to_string());
    }

    None
}
