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
        .find([':', ' ', '\t'])
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

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            commands: HashMap::new(),
        };
        registry.register(Arc::new(ModelCommand));
        registry.register(Arc::new(ResetCommand));
        registry.register(Arc::new(ProviderCommand));
        registry.register(Arc::new(TempCommand));
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
        "Show or switch AI provider. Usage: /provider [list | set <name> <key>]"
    }
    fn execute(&self, agent: &mut Agent, arg: &str) -> Option<String> {
        let parts: Vec<&str> = arg.split_whitespace().collect();

        if parts.is_empty() || parts[0] == "list" {
            let mut msg = String::from("Configured providers:\n");
            if let Some(path) = &agent.config_path
                && let Ok(content) = std::fs::read_to_string(path)
                    && let Ok(cfg) = serde_json::from_str::<crate::config::Config>(&content) {
                        if let Some(models) = cfg.models {
                            for name in models.providers.keys() {
                                let marker = if name == &cfg.default_provider {
                                    " (current)"
                                } else {
                                    ""
                                };
                                msg.push_str(&format!("- {}{}\n", name, marker));
                            }
                        }
                        return Some(msg);
                    }
            return Some("Could not list providers: config not found.".to_string());
        }

        if parts[0] == "set" && parts.len() >= 2 {
            let provider_name = parts[1];
            let api_key = if parts.len() >= 3 {
                Some(parts[2].to_string())
            } else {
                None
            };

            if let Some(path) = &agent.config_path {
                match std::fs::read_to_string(path) {
                    Ok(content) => {
                        match serde_json::from_str::<crate::config::Config>(&content) {
                            Ok(mut cfg) => {
                                // 1. Update or Add the provider config
                                if let Some(key) = api_key {
                                    if let Some(models) = &mut cfg.models {
                                        let p_cfg = models
                                            .providers
                                            .entry(provider_name.to_string())
                                            .or_insert(crate::config::ProviderConfig {
                                                api_key: key.clone(),
                                                base_url: None,
                                                model: None,
                                            });
                                        p_cfg.api_key = key;
                                    } else {
                                        let mut providers = std::collections::HashMap::new();
                                        providers.insert(
                                            provider_name.to_string(),
                                            crate::config::ProviderConfig {
                                                api_key: key,
                                                base_url: None,
                                                model: None,
                                            },
                                        );
                                        cfg.models =
                                            Some(crate::config::ModelsConfig { providers });
                                    }
                                }

                                // 2. Update default provider
                                cfg.default_provider = provider_name.to_string();
                                cfg.config_path = path.clone();

                                // 3. Save config
                                if let Err(e) = cfg.save() {
                                    return Some(format!("Failed to save config: {}", e));
                                }

                                // 4. Update Agent's provider runtime
                                use crate::providers::factory;
                                agent.provider =
                                    factory::create_with_fallbacks(provider_name, &cfg);

                                return Some(format!(
                                    "Switched to provider: {}. Config saved.",
                                    provider_name
                                ));
                            }
                            Err(e) => return Some(format!("Failed to parse config: {}", e)),
                        }
                    }
                    Err(e) => return Some(format!("Failed to read config: {}", e)),
                }
            }
            return Some("Config path not found.".to_string());
        }

        Some("Usage: /provider [list | set <name> <key>]".to_string())
    }
}

struct TempCommand;
impl Command for TempCommand {
    fn name(&self) -> &str {
        "temp"
    }
    fn description(&self) -> &str {
        "Show system temperature (CPU/GPU)"
    }
    fn execute(&self, _agent: &mut Agent, _arg: &str) -> Option<String> {
        use crate::hardware::get_system_temperature;
        match get_system_temperature() {
            Ok(t) => Some(format!("System Temperature: {}", t)),
            Err(e) => Some(format!("Error fetching temperature: {}", e)),
        }
    }
}

pub fn bare_session_reset_prompt(message: &str) -> Option<&'static str> {
    if let Some(cmd) = parse_slash_command(message)
        && (cmd.name.eq_ignore_ascii_case("new") || cmd.name.eq_ignore_ascii_case("reset"))
            && cmd.arg.is_empty()
        {
            return Some(BARE_SESSION_RESET_PROMPT);
        }
    None
}

pub fn handle_slash_command(agent: &mut Agent, message: &str) -> Option<String> {
    // Legacy bridge to the new registry
    CommandRegistry::handle_message(agent, message)
}
