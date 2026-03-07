use crate::agent::Agent;
use std::collections::HashMap;
use std::sync::Arc;

pub const BARE_SESSION_RESET_PROMPT: &str = "A new session was started via /new or /reset. Execute your Session Startup sequence now - read the required files before responding to the user. Then greet the user in your configured persona, if one is provided. Be yourself - use your defined voice, mannerisms, and mood. Keep it to 1-3 sentences and ask what they want to do. If the runtime model differs from default_model in the system prompt, mention the default model. Do not mention internal steps, files, tools, or reasoning.";

#[derive(Debug, PartialEq, Clone)]
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
    let split_idx = body
        .find(|c| c == ':' || c == ' ' || c == '\t')
        .unwrap_or(body.len());

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

pub trait Command: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn execute(&self, agent: &mut Agent, arg: &str) -> Option<String>;
}

pub struct CommandRegistry {
    commands: HashMap<String, Arc<dyn Command>>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            commands: HashMap::new(),
        };
        registry.register(Arc::new(ModelCommand));
        registry.register(Arc::new(ResetCommand));
        registry.register(Arc::new(ProviderCommand));
        registry
    }

    pub fn register(&mut self, cmd: Arc<dyn Command>) {
        self.commands.insert(cmd.name().to_lowercase(), cmd);
    }

    pub fn handle_message(agent: &mut Agent, message: &str) -> Option<String> {
        let parsed = parse_slash_command(message)?;
        let cmd = {
            let registry = &agent.command_registry;
            registry.commands.get(&parsed.name.to_lowercase()).cloned()
        };

        if let Some(cmd) = cmd {
            return cmd.execute(agent, &parsed.arg);
        }
        None
    }
}

struct ModelCommand;
impl Command for ModelCommand {
    fn name(&self) -> &str {
        "model"
    }
    fn description(&self) -> &str {
        "Show or change the current model"
    }
    fn execute(&self, agent: &mut Agent, arg: &str) -> Option<String> {
        if arg.is_empty() {
            return Some(format!("Current model: {}", agent.model_name));
        }
        agent.model_name = arg.to_string();
        // Recalculate token limits
        use crate::agent::context_tokens::resolve_context_tokens;
        use crate::agent::max_tokens::resolve_max_tokens;

        agent.token_limit = resolve_context_tokens(None, &agent.model_name);
        agent.max_tokens = resolve_max_tokens(None, &agent.model_name);

        Some(format!("Switched model to: {}", agent.model_name))
    }
}

struct ResetCommand;
impl Command for ResetCommand {
    fn name(&self) -> &str {
        "reset"
    }
    fn description(&self) -> &str {
        "Reset the current session"
    }
    fn execute(&self, agent: &mut Agent, _arg: &str) -> Option<String> {
        agent.reset_history();
        agent.memory_session_id = Some("new-session".to_string());
        Some("Session reset.".to_string())
    }
}

struct ProviderCommand;
impl Command for ProviderCommand {
    fn name(&self) -> &str {
        "provider"
    }
    fn description(&self) -> &str {
        "Switch AI provider"
    }
    fn execute(&self, agent: &mut Agent, _arg: &str) -> Option<String> {
        Some(format!(
            "Provider switching not fully implemented. Current model: {}",
            agent.model_name
        ))
    }
}

pub fn bare_session_reset_prompt(message: &str) -> Option<&'static str> {
    if let Some(cmd) = parse_slash_command(message) {
        if (cmd.name.eq_ignore_ascii_case("new") || cmd.name.eq_ignore_ascii_case("reset"))
            && cmd.arg.is_empty()
        {
            return Some(BARE_SESSION_RESET_PROMPT);
        }
    }
    None
}

pub fn handle_slash_command(agent: &mut Agent, message: &str) -> Option<String> {
    // Legacy bridge to the new registry
    CommandRegistry::handle_message(agent, message)
}
