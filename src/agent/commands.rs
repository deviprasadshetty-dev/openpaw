use crate::agent::Agent;
use std::collections::HashMap;
use std::sync::Arc;

pub const BARE_SESSION_RESET_PROMPT: &str = "A new session was started via /new or /reset. Execute your Session Startup sequence now - read the relevant files and orient yourself.";

#[derive(Debug, Clone)]
pub struct SlashCommand {
    pub name: String,
    pub arg: String,
}

pub fn parse_slash_command(message: &str) -> Option<SlashCommand> {
    let body = message.trim();
    if !body.starts_with('/') && !body.starts_with('!') {
        return None;
    }

    let body = &body[1..];
    let split_idx = body.find([':', ' ', '\t']).unwrap_or(body.len());

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
        registry.register(Arc::new(SwitchCommand));
        registry.register(Arc::new(TempCommand));
        registry.register(Arc::new(CuratorCommand));
        registry.register(Arc::new(InsightsCommand));
        registry.register(Arc::new(UsageCommand));
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
        "Show or switch AI provider. Usage: /provider [list | <name> | set <name> <key>]"
    }
    fn execute(&self, agent: &mut Agent, arg: &str) -> Option<String> {
        let parts: Vec<&str> = arg.split_whitespace().collect();

        if parts.is_empty() || parts[0] == "list" {
            let mut msg = String::from("Configured providers:\n");
            if let Some(path) = &agent.config_path
                && let Ok(content) = std::fs::read_to_string(path)
                && let Ok(cfg) = serde_json::from_str::<crate::config::Config>(&content)
            {
                let mut sorted_keys: Vec<_> = if let Some(models) = &cfg.models {
                    models.providers.keys().collect()
                } else {
                    Vec::new()
                };
                sorted_keys.sort();

                for name in sorted_keys {
                    let marker = if name == &cfg.default_provider {
                        " (current)"
                    } else {
                        ""
                    };
                    msg.push_str(&format!("- {}{}\n", name, marker));
                }
                return Some(msg);
            }
            return Some("Could not list providers: config not found.".to_string());
        }

        let (provider_name, api_key) = if parts[0] == "set" && parts.len() >= 2 {
            (
                parts[1],
                if parts.len() >= 3 {
                    Some(parts[2].to_string())
                } else {
                    None
                },
            )
        } else {
            (parts[0], None)
        };

        if let Some(path) = &agent.config_path {
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    match serde_json::from_str::<crate::config::Config>(&content) {
                        Ok(mut cfg) => {
                            // 1. Update or Add the provider config if key provided
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
                                    cfg.models = Some(crate::config::ModelsConfig { providers });
                                }
                            }

                            // Verify provider exists in config
                            if cfg
                                .models
                                .as_ref()
                                .and_then(|m| m.providers.get(provider_name))
                                .is_none()
                                && provider_name != "ollama"
                                && provider_name != "lmstudio"
                            {
                                return Some(format!(
                                    "Provider '{}' not found in config. Use '/provider set {} <key>' first.",
                                    provider_name, provider_name
                                ));
                            }

                            // 2. Update default provider in config
                            cfg.default_provider = provider_name.to_string();
                            cfg.config_path = path.clone();

                            // 3. Sync Agent's model for the new provider
                            if let Some(new_model) = cfg.get_model_for_provider(provider_name) {
                                agent.model_name = new_model;
                                // Recalculate limits
                                use crate::agent::context_tokens::resolve_context_tokens;
                                use crate::agent::max_tokens::resolve_max_tokens;
                                agent.token_limit = resolve_context_tokens(None, &agent.model_name);
                                agent.max_tokens = resolve_max_tokens(None, &agent.model_name);
                            }

                            // 4. Save config
                            if let Err(e) = cfg.save() {
                                return Some(format!("Failed to save config: {}", e));
                            }

                            // 5. Update Agent's provider runtime
                            use crate::providers::factory;
                            agent.provider = factory::create_with_fallbacks(provider_name, &cfg);

                            return Some(format!(
                                "Switched to provider: {} (model: {}). Config saved.",
                                provider_name, agent.model_name
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
}

struct SwitchCommand;
impl Command for SwitchCommand {
    fn name(&self) -> &str {
        "switch"
    }
    fn description(&self) -> &str {
        "Shortcut to switch provider. Usage: /switch <provider_name>"
    }
    fn execute(&self, agent: &mut Agent, arg: &str) -> Option<String> {
        let provider_cmd = ProviderCommand;
        provider_cmd.execute(agent, arg)
    }
}

struct CuratorCommand;
impl Command for CuratorCommand {
    fn name(&self) -> &str {
        "curator"
    }
    fn description(&self) -> &str {
        "Curator controls. Usage: /curator [status | run | pause | resume]"
    }
    fn execute(&self, agent: &mut Agent, arg: &str) -> Option<String> {
        let usage_db = agent.skill_usage_db.clone();

        match arg.trim() {
            "run" | "" => {
                // Trigger curator check immediately
                agent.last_curator_check = 0;
                agent.maybe_run_curator();

                // Also show current stats
                if let Some(ref db) = usage_db {
                    match db.full_report() {
                        Ok(report) => {
                            let total = report.len();
                            let active = report.iter().filter(|r| r.state == "active").count();
                            let stale = report.iter().filter(|r| r.state == "stale").count();
                            let archived = report.iter().filter(|r| r.state == "archived").count();
                            return Some(format!(
                                "Curator check triggered.\nSkill stats: {} total ({} active, {} stale, {} archived)\nFull report will be written to state/curator_reports/",
                                total, active, stale, archived
                            ));
                        }
                        Err(e) => return Some(format!("Curator triggered but stats query failed: {}", e)),
                    }
                }
                Some("Curator check triggered (no usage DB available).".to_string())
            }
            "status" => {
                if let Some(ref db) = usage_db {
                    match db.agent_created_report() {
                        Ok(report) if report.is_empty() => {
                            return Some("No agent-created skills yet. Skills are created when the agent solves complex tasks.".to_string());
                        }
                        Ok(report) => {
                            let mut lines = vec![format!("Agent-created skills: {}", report.len())];
                            for row in &report {
                                lines.push(crate::skills::usage::format_skill_row(row));
                            }
                            return Some(lines.join("\n"));
                        }
                        Err(e) => return Some(format!("Failed to query skill stats: {}", e)),
                    }
                }
                Some("Skill usage database not available (curator requires daemon mode).".to_string())
            }
            "pause" => {
                if let Some(ref db) = usage_db {
                    // Mark a special sentinel skill as the pause indicator
                    let _ = db.set_pinned("_curator_paused", true);
                }
                agent.curator_config.enabled = false;
                Some("Curator paused. Use /curator resume to re-enable.".to_string())
            }
            "resume" => {
                agent.curator_config.enabled = true;
                agent.last_curator_check = 0; // Allow immediate check
                Some("Curator resumed.".to_string())
            }
            _ => Some("Usage: /curator [status | run | pause | resume]".to_string()),
        }
    }
}

struct InsightsCommand;
impl Command for InsightsCommand {
    fn name(&self) -> &str {
        "insights"
    }
    fn description(&self) -> &str {
        "Show usage insights: tokens, costs, tool patterns, cost-saving stats"
    }
    fn execute(&self, agent: &mut Agent, _arg: &str) -> Option<String> {
        let mut lines = vec!["━━━ Agent Insights ━━━".to_string()];

        // Model & provider
        lines.push(format!("Model: {}", agent.model_name));
        lines.push(format!("Provider: {}", agent.provider.get_name()));
        lines.push(format!("Token limit: {}", agent.token_limit));

        // Token usage
        lines.push(format!("Total tokens: {}", agent.total_tokens));
        lines.push(format!("Turns this session: {}", agent.user_turn_count));
        lines.push(format!("Tool iterations: {}", agent.iters_since_skill));

        // Compaction savings
        if let Some(ref compressor) = agent.context_compressor {
            lines.push(format!(
                "Compactions: {} (savings: {:.0}%, ineffective streak: {})",
                compressor.compression_count,
                compressor.last_compression_savings_pct,
                compressor.ineffective_compression_count
            ));
        }

        // Cost tracking
        if let Some(ref tracker) = agent.cost_tracker {
            let session_cost = tracker.session_cost();
            let (input_price, output_price) =
                crate::token_estimator::get_model_prices(&agent.model_name);
            lines.push("\nCost tracking:".to_string());
            lines.push(format!(
                "  Session cost: ${:.4}",
                session_cost
            ));
            lines.push(format!(
                "  Model pricing: ${:.2}/M in, ${:.2}/M out",
                input_price, output_price
            ));
            let est_total = crate::token_estimator::estimate_cost(
                &agent.model_name,
                agent.total_tokens / 2,
                agent.total_tokens / 2,
            );
            lines.push(format!(
                "  Estimated all-time: ${:.4}",
                est_total.max(session_cost)
            ));

            // Budget status
            match tracker.check_budget(0.0) {
                crate::cost::BudgetCheck::Warning(info) => {
                    lines.push(format!(
                        "  ⚠ Budget: ${:.4} / ${:.2} ({:?})",
                        info.current_usd, info.limit_usd, info.period
                    ));
                }
                crate::cost::BudgetCheck::Exceeded(info) => {
                    lines.push(format!(
                        "  🔴 Over budget: ${:.4} / ${:.2} ({:?})",
                        info.current_usd, info.limit_usd, info.period
                    ));
                }
                _ => {}
            }
        }

        // Skill journal stats
        if let Some(ref journal) = agent.skill_journal {
            let summary = journal.improvement_summary(1);
            if let Some(s) = summary {
                lines.push("\nSkill execution feedback:".to_string());
                lines.push(s);
            } else {
                lines.push("\nSkills: no failures recorded.".to_string());
            }
        }

        // Curator / skill DB stats
        if let Some(ref db) = agent.skill_usage_db {
            match db.full_report() {
                Ok(report) => {
                    let total = report.len();
                    let active = report.iter().filter(|r| r.state == "active").count();
                    lines.push(format!("\nSkill DB: {} total ({} active)", total, active));
                }
                _ => {}
            }
        }

        // Cost-saving features status
        lines.push("\nCost-saving features:".to_string());
        lines.push(format!(
            "  Compression: {}",
            if agent.context_compressor.is_some() {
                "active"
            } else {
                "disabled"
            }
        ));
        lines.push(format!(
            "  Prompt caching: {}",
            if agent.efficiency_config.prompt_caching {
                "enabled"
            } else {
                "off"
            }
        ));
        lines.push(format!(
            "  Cheap provider: {}",
            if agent.cheap_provider.is_some() {
                "configured"
            } else {
                "not configured"
            }
        ));
        lines.push("  Response cache: active (1h TTL)".to_string());
        lines.push("  Reasoning scrub: active".to_string());

        lines.push("━━━━━━━━━━━━━━━━━━━━".to_string());
        Some(lines.join("\n"))
    }
}

struct UsageCommand;
impl Command for UsageCommand {
    fn name(&self) -> &str {
        "usage"
    }
    fn description(&self) -> &str {
        "Show token usage and cost report for this session"
    }
    fn execute(&self, agent: &mut Agent, _arg: &str) -> Option<String> {
        let mut lines = vec!["━━━ Session Usage Report ━━━".to_string()];

        // Model info
        lines.push(format!("Model: {}", agent.model_name));
        lines.push(format!("Context window: {} tokens", agent.token_limit));
        lines.push(format!("Max output tokens: {}", agent.max_tokens));

        // Current session tokens
        lines.push(format!(
            "Total tokens this session: {} (prompt + completion)",
            agent.total_tokens
        ));
        lines.push(format!(
            "Tokens this turn: {}",
            agent.current_turn_tokens
        ));

        // Conversation size
        let history_tokens =
            crate::token_estimator::estimate_history_tokens_rough(&agent.history);
        lines.push(format!(
            "Current history: {} msgs, ~{} tokens",
            agent.history.len(),
            history_tokens
        ));

        // Context utilization
        if agent.token_limit > 0 {
            let pct = (history_tokens as f64 / agent.token_limit as f64 * 100.0).min(100.0);
            lines.push(format!("Context used: {:.0}% of window", pct));
        }

        // Compaction stats
        if let Some(ref compressor) = agent.context_compressor {
            if compressor.compression_count > 0 {
                lines.push(format!(
                    "Compactions: {} (last saved {:.0}%)",
                    compressor.compression_count, compressor.last_compression_savings_pct
                ));
            }
        }

        // Cost tracking
        if let Some(ref tracker) = agent.cost_tracker {
            let session_cost = tracker.session_cost();
            let (input_price, output_price) =
                crate::token_estimator::get_model_prices(&agent.model_name);
            let est_cost_all = crate::token_estimator::estimate_cost(
                &agent.model_name,
                agent.total_tokens / 2, // rough split
                agent.total_tokens / 2,
            );
            lines.push(format!(
                "Estimated cost this session: ${:.4}",
                session_cost.max(est_cost_all)
            ));
            lines.push(format!(
                "Model pricing: ${:.2}/M input, ${:.2}/M output",
                input_price, output_price
            ));

            // Budget check
            let budget = tracker.check_budget(0.0);
            match budget {
                crate::cost::BudgetCheck::Allowed => {
                    lines.push("Budget: within limits".to_string());
                }
                crate::cost::BudgetCheck::Warning(info) => {
                    lines.push(format!(
                        "⚠ Budget warning: ${:.4} / ${:.2} ({:?})",
                        info.current_usd, info.limit_usd, info.period
                    ));
                }
                crate::cost::BudgetCheck::Exceeded(info) => {
                    lines.push(format!(
                        "🔴 Budget exceeded: ${:.4} / ${:.2} ({:?})",
                        info.current_usd, info.limit_usd, info.period
                    ));
                }
            }
        }

        // Compressor stats
        if let Some(ref compressor) = agent.context_compressor {
            let threshold = compressor.threshold_tokens;
            let tail_budget = compressor.tail_token_budget;
            lines.push(format!(
                "Compression threshold: {} tokens, tail budget: {}",
                threshold, tail_budget
            ));
        }

        // Response cache stats (rough)
        lines.push(
            "Response cache: active (1h TTL)".to_string(),
        );

        lines.push("━━━━━━━━━━━━━━━━━━━━━━━━━".to_string());
        Some(lines.join("\n"))
    }
}

struct TempCommand;
impl Command for TempCommand {
    fn name(&self) -> &str {
        "temp"
    }
    fn description(&self) -> &str {
        "Change the sampling temperature (0.0 to 2.0)"
    }
    fn execute(&self, agent: &mut Agent, arg: &str) -> Option<String> {
        if let Ok(temp) = arg.parse::<f32>() {
            if (0.0..=2.0).contains(&temp) {
                agent.temperature = temp;
                return Some(format!("Temperature set to: {}", temp));
            }
        }
        Some("Usage: /temp <0.0-2.0>".to_string())
    }
}

pub fn handle_message(agent: &mut Agent, message: &str) -> Option<String> {
    if message.trim().to_lowercase() == "/new" || message.trim().to_lowercase() == "/reset" {
        agent.reset_history();
        return Some(BARE_SESSION_RESET_PROMPT.to_string());
    }
    CommandRegistry::handle_message(agent, message)
}
