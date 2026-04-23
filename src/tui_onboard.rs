use std::{
    io::stdout,
    time::Duration,
};

use anyhow::Result;
use crossbeam_channel::{bounded, Receiver};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph, Row, Table,
        Wrap,
    },
    Frame, Terminal,
};
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;

use crate::onboard::{
    fetch_anthropic_models, fetch_gemini_models, fetch_ollama_compat_models, fetch_ollama_models,
    fetch_openai_compat_models, fetch_openai_models, PartialConfig, ProviderConfig,
    COMMON_TIMEZONES,
};
use crate::providers::kilocode::fetch_kilocode_free_models;
use crate::providers::openrouter::{
    fetch_openrouter_free_models, format_openrouter_model, preferred_openrouter_model_index,
    OpenRouterFreeModel,
};

// ═════════════════════════════════════════════════════════════════════════════
// Theme
// ═════════════════════════════════════════════════════════════════════════════

struct Theme {
    bg: Color,
    fg: Color,
    primary: Color,
    success: Color,
    warning: Color,
    error: Color,
    muted: Color,
    border: Color,
    highlight_bg: Color,
    highlight_fg: Color,
}

const THEME: Theme = Theme {
    bg: Color::Black,
    fg: Color::Gray,
    primary: Color::Cyan,
    success: Color::Green,
    warning: Color::Yellow,
    error: Color::Red,
    muted: Color::DarkGray,
    border: Color::Gray,
    highlight_bg: Color::DarkGray,
    highlight_fg: Color::White,
};

// ═════════════════════════════════════════════════════════════════════════════
// Wizard steps
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    Welcome,
    Provider,
    CheapModel,
    Timezone,
    Memory,
    Voice,
    Channels,
    Composio,
    WebSearch,
    Pushover,
    Summary,
    Done,
}

impl Step {
    fn label(&self) -> &'static str {
        match self {
            Step::Welcome => "Welcome",
            Step::Provider => "AI Provider",
            Step::CheapModel => "Background Tasks",
            Step::Timezone => "Timezone",
            Step::Memory => "Memory",
            Step::Voice => "Voice",
            Step::Channels => "Channels",
            Step::Composio => "Composio",
            Step::WebSearch => "Web Search",
            Step::Pushover => "Pushover",
            Step::Summary => "Summary",
            Step::Done => "Done",
        }
    }

    fn all() -> &'static [Step] {
        &[
            Step::Provider,
            Step::CheapModel,
            Step::Timezone,
            Step::Memory,
            Step::Voice,
            Step::Channels,
            Step::Composio,
            Step::WebSearch,
            Step::Pushover,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderStage {
    Select,
    GeminiAuth,
    Key,
    BaseUrl,
    Fetching,
    ModelSelect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheapStage {
    Select,
    Key,
    Fetching,
    ModelSelect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryStage {
    SelectBackend,
    ConfirmEmbed,
    EmbedKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceStage {
    Confirm,
    Key,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelStage {
    Select,
    Telegram,
    Whatsapp,
    Email,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposioStage {
    Confirm,
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebSearchStage {
    Select,
    BraveKey,
    GeminiCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PushoverStage {
    Confirm,
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubStage {
    None,
    Provider(ProviderStage),
    Cheap(CheapStage),
    Memory(MemoryStage),
    Voice(VoiceStage),
    Channel(ChannelStage),
    Composio(ComposioStage),
    WebSearch(WebSearchStage),
    Pushover(PushoverStage),
}

// ═════════════════════════════════════════════════════════════════════════════
// Fetching
// ═════════════════════════════════════════════════════════════════════════════

enum FetchResult {
    Strings(Vec<String>),
    OpenRouterModels(Vec<OpenRouterFreeModel>),
    Error(String),
}

enum FetchKind {
    Gemini(String),
    OpenAi(String),
    Anthropic(String),
    OpenRouter(String),
    OpenCode(String),
    Kilocode(String),
    Ollama,
    LmStudio,
    Custom(String, String),
    CheapOpenRouter(String),
    CheapOpenCode(String),
    CheapKilocode(String),
}

// ═════════════════════════════════════════════════════════════════════════════
// App
// ═════════════════════════════════════════════════════════════════════════════

pub struct App {
    config: PartialConfig,
    is_edit: bool,
    should_save: bool,
    should_quit: bool,

    current_step: Step,
    sub_stage: SubStage,

    list_state: ListState,
    summary_list: ListState,
    inputs: Vec<Input>,
    focused_input: usize,

    providers: Vec<(&'static str, &'static str, &'static str, &'static str)>,
    cheap_providers: Vec<(&'static str, &'static str)>,

    model_ids: Vec<String>,
    model_labels: Vec<String>,

    fetching: bool,
    fetch_rx: Option<Receiver<FetchResult>>,

    toast: Option<String>,
    toast_ticks: u8,
    show_help: bool,
    tick: u64,

    // transient provider selection
    selected_provider_idx: usize,
    selected_cheap_provider_idx: usize,
    gemini_auth_choice: usize,
}

impl App {
    pub fn new(config: PartialConfig, is_edit: bool) -> Self {
        let providers = vec![
            ("gemini", "Gemini", "Google", "gemini-2.0-flash"),
            ("openai", "GPT-4o", "OpenAI", "gpt-4o"),
            ("anthropic", "Claude", "Anthropic", "claude-sonnet-4-5"),
            (
                "openrouter",
                "OpenRouter",
                "OpenRouter",
                "deepseek/deepseek-chat-v3-0324:free",
            ),
            ("opencode", "OpenCode", "OpenCode", "minimax-m2.5-free"),
            (
                "kilocode",
                "Kilocode",
                "Kilo.ai",
                "minimax/minimax-m2.1:free",
            ),
            ("ollama", "Ollama", "Local", "llama3.2"),
            ("lmstudio", "LM Studio", "Local", "local-model"),
            ("openai-compatible", "Custom / Compatible", "Custom", ""),
        ];
        let cheap_providers = vec![
            ("openrouter", "OpenRouter (Free)"),
            ("opencode", "OpenCode (Free)"),
            ("kilocode", "Kilocode (Free)"),
            ("none", "None — Use primary model"),
        ];

        let mut app = Self {
            config,
            is_edit,
            should_save: false,
            should_quit: false,
            current_step: if is_edit { Step::Summary } else { Step::Welcome },
            sub_stage: SubStage::None,
            list_state: ListState::default(),
            summary_list: ListState::default(),
            inputs: Vec::new(),
            focused_input: 0,
            providers,
            cheap_providers,
            model_ids: Vec::new(),
            model_labels: Vec::new(),
            fetching: false,
            fetch_rx: None,
            toast: None,
            toast_ticks: 0,
            show_help: false,
            tick: 0,
            selected_provider_idx: 0,
            selected_cheap_provider_idx: 0,
            gemini_auth_choice: 0,
        };
        app.init_selections();
        if is_edit {
            // Default to "Save & Exit" so returning users can press Enter immediately
            app.summary_list.select(Some(9));
        }
        app
    }

    fn init_selections(&mut self) {
        // Provider
        if let Some(p) = &self.config.provider {
            if let Some(idx) = self.providers.iter().position(|(id, ..)| *id == p.name) {
                self.list_state.select(Some(idx));
                self.selected_provider_idx = idx;
            }
        } else {
            self.list_state.select(Some(0));
        }

        // Cheap provider
        if let Some(Some(cp)) = &self.config.cheap_provider {
            if let Some(idx) = self.cheap_providers.iter().position(|(id, _)| *id == cp) {
                self.selected_cheap_provider_idx = idx;
            }
        }
    }

    /// Return the index of the currently-configured model inside `self.model_ids`,
    /// or `None` if it isn't present.
    fn find_current_model_index(&self, cheap: bool) -> Option<usize> {
        let target = if cheap {
            self.config.cheap_model.as_ref().and_then(|m| m.as_deref())
        } else {
            self.config
                .provider
                .as_ref()
                .and_then(|p| p.model.as_ref().map(|s| s.as_str()))
                .or(self.config.selected_default_model.as_deref())
        };
        target.and_then(|t| self.model_ids.iter().position(|m| m == t))
    }

    pub fn run(mut self) -> Result<(bool, PartialConfig)> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let tick_rate = Duration::from_millis(100);
        let mut last_tick = std::time::Instant::now();

        let result = loop {
            terminal.draw(|f| self.draw(f))?;

            if self.should_quit {
                break Ok(());
            }

            let timeout = tick_rate.saturating_sub(last_tick.elapsed());
            if crossterm::event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        if self.handle_key(key).is_break() {
                            break Ok(());
                        }
                    }
                }
            }

            if last_tick.elapsed() >= tick_rate {
                self.on_tick();
                last_tick = std::time::Instant::now();
            }
        };

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;

        result.map(|_| (self.should_save, self.config))
    }

    fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self.toast_ticks > 0 {
            self.toast_ticks -= 1;
            if self.toast_ticks == 0 {
                self.toast = None;
            }
        }

        if self.fetching {
            if let Some(rx) = &self.fetch_rx {
                if let Ok(result) = rx.try_recv() {
                    self.fetching = false;
                    self.fetch_rx = None;
                    self.handle_fetch_result(result);
                }
            }
        }
    }

    fn show_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some(msg.into());
        self.toast_ticks = 40; // 4 seconds
    }

    fn handle_fetch_result(&mut self, result: FetchResult) {
        match self.current_step {
            Step::Provider => match result {
                FetchResult::Strings(v) => {
                    self.model_ids = v.clone();
                    self.model_labels = v;
                    let idx = self.find_current_model_index(false).unwrap_or(0);
                    self.list_state.select(Some(idx));
                    self.sub_stage = SubStage::Provider(ProviderStage::ModelSelect);
                }
                FetchResult::OpenRouterModels(v) => {
                    self.model_ids = v.iter().map(|m| m.id.clone()).collect();
                    self.model_labels = v.iter().map(|m| format_openrouter_model(m)).collect();
                    let idx = self
                        .find_current_model_index(false)
                        .unwrap_or_else(|| preferred_openrouter_model_index(&v));
                    self.list_state.select(Some(idx));
                    self.sub_stage = SubStage::Provider(ProviderStage::ModelSelect);
                }
                FetchResult::Error(e) => {
                    self.show_toast(format!("Model fetch failed: {}", e));
                    self.sub_stage = SubStage::Provider(ProviderStage::ModelSelect);
                    // fallback: use default model as single option
                    let default = self
                        .providers
                        .get(self.selected_provider_idx)
                        .map(|(_, _, _, m)| m.to_string())
                        .unwrap_or_default();
                    self.model_ids = vec![default.clone()];
                    self.model_labels = vec![default];
                    self.list_state.select(Some(0));
                }
            },
            Step::CheapModel => match result {
                FetchResult::Strings(v) => {
                    self.model_ids = v.clone();
                    self.model_labels = v;
                    let idx = self.find_current_model_index(true).unwrap_or(0);
                    self.list_state.select(Some(idx));
                    self.sub_stage = SubStage::Cheap(CheapStage::ModelSelect);
                }
                FetchResult::OpenRouterModels(v) => {
                    self.model_ids = v.iter().map(|m| m.id.clone()).collect();
                    self.model_labels = v.iter().map(|m| format_openrouter_model(m)).collect();
                    let idx = self
                        .find_current_model_index(true)
                        .unwrap_or_else(|| preferred_openrouter_model_index(&v));
                    self.list_state.select(Some(idx));
                    self.sub_stage = SubStage::Cheap(CheapStage::ModelSelect);
                }
                FetchResult::Error(e) => {
                    self.show_toast(format!("Model fetch failed: {}", e));
                    self.sub_stage = SubStage::Cheap(CheapStage::ModelSelect);
                    let default = match self.cheap_providers[self.selected_cheap_provider_idx].0 {
                        "openrouter" => "deepseek/deepseek-chat-v3-0324:free".to_string(),
                        "opencode" => "minimax-m2.5-free".to_string(),
                        "kilocode" => "minimax/minimax-m2.1:free".to_string(),
                        _ => String::new(),
                    };
                    self.model_ids = vec![default.clone()];
                    self.model_labels = vec![default];
                    self.list_state.select(Some(0));
                }
            },
            _ => {}
        }
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Event handling
    // ═════════════════════════════════════════════════════════════════════════

    fn handle_key(&mut self, key: KeyEvent) -> std::ops::ControlFlow<()> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return std::ops::ControlFlow::Break(());
        }

        if key.code == KeyCode::Char('?') {
            self.show_help = !self.show_help;
            return std::ops::ControlFlow::Continue(());
        }

        if self.show_help {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?')) {
                self.show_help = false;
            }
            return std::ops::ControlFlow::Continue(());
        }

        match self.current_step {
            Step::Welcome => self.handle_welcome(key),
            Step::Provider => self.handle_provider(key),
            Step::CheapModel => self.handle_cheap_model(key),
            Step::Timezone => self.handle_timezone(key),
            Step::Memory => self.handle_memory(key),
            Step::Voice => self.handle_voice(key),
            Step::Channels => self.handle_channels(key),
            Step::Composio => self.handle_composio(key),
            Step::WebSearch => self.handle_websearch(key),
            Step::Pushover => self.handle_pushover(key),
            Step::Summary => self.handle_summary(key),
            Step::Done => self.handle_done(key),
        }
    }

    fn handle_welcome(&mut self, key: KeyEvent) -> std::ops::ControlFlow<()> {
        match key.code {
            KeyCode::Enter => {
                if self.is_edit {
                    self.current_step = Step::Summary;
                } else {
                    self.current_step = Step::Provider;
                    self.sub_stage = SubStage::Provider(ProviderStage::Select);
                    self.init_selections();
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.should_quit = true;
            }
            _ => {}
        }
        std::ops::ControlFlow::Continue(())
    }

    fn handle_provider(&mut self, key: KeyEvent) -> std::ops::ControlFlow<()> {
        if self.fetching {
            if key.code == KeyCode::Esc {
                self.fetching = false;
                self.fetch_rx = None;
                self.sub_stage = SubStage::Provider(ProviderStage::Select);
            }
            return std::ops::ControlFlow::Continue(());
        }

        match self.sub_stage {
            SubStage::Provider(ProviderStage::Select) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.list_up(),
                KeyCode::Down | KeyCode::Char('j') => self.list_down(),
                KeyCode::Enter => {
                    self.selected_provider_idx = self.list_state.selected().unwrap_or(0);
                    let (id, ..) = self.providers[self.selected_provider_idx];
                    match id {
                        "gemini" => {
                            self.sub_stage = SubStage::Provider(ProviderStage::GeminiAuth);
                            self.gemini_auth_choice = 0;
                            self.list_state.select(Some(0));
                        }
                        "ollama" | "lmstudio" => {
                            self.config.api_key = Some(String::new());
                            if id == "ollama" {
                                self.config.provider = Some(ProviderConfig {
                                    name: "ollama".to_string(),
                                    default_model: "llama3.2".to_string(),
                                    base_url: Some("http://localhost:11434/v1".to_string()),
                                    model: None,
                                });
                                self.spawn_fetch(FetchKind::Ollama);
                            } else {
                                self.config.provider = Some(ProviderConfig {
                                    name: "lmstudio".to_string(),
                                    default_model: "local-model".to_string(),
                                    base_url: Some("http://localhost:1234/v1".to_string()),
                                    model: None,
                                });
                                self.spawn_fetch(FetchKind::LmStudio);
                            }
                            self.sub_stage = SubStage::Provider(ProviderStage::Fetching);
                        }
                        "openai-compatible" => {
                            self.inputs = vec![Input::new(
                                self.config
                                    .custom_base_url
                                    .as_ref()
                                    .and_then(|u| u.as_ref())
                                    .cloned()
                                    .unwrap_or_else(|| "http://localhost:8080/v1".to_string()),
                            )];
                            self.focused_input = 0;
                            self.sub_stage = SubStage::Provider(ProviderStage::BaseUrl);
                        }
                        _ => {
                            let existing = self.config.api_key.clone().unwrap_or_default();
                            self.inputs = vec![Input::new(existing)];
                            self.focused_input = 0;
                            self.sub_stage = SubStage::Provider(ProviderStage::Key);
                        }
                    }
                }
                KeyCode::Esc => self.go_back(),
                _ => {}
            },
            SubStage::Provider(ProviderStage::GeminiAuth) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.list_up(),
                KeyCode::Down | KeyCode::Char('j') => self.list_down(),
                KeyCode::Enter => {
                    self.gemini_auth_choice = self.list_state.selected().unwrap_or(0);
                    if self.gemini_auth_choice == 1 {
                        self.config.api_key = Some("cli_oauth".to_string());
                        self.config.provider = Some(ProviderConfig {
                            name: "gemini".to_string(),
                            default_model: "gemini-2.0-flash".to_string(),
                            base_url: None,
                            model: Some("gemini-2.0-flash".to_string()),
                        });
                        self.config.selected_default_model = Some("gemini-2.0-flash".to_string());
                        self.advance_step();
                    } else {
                        let existing = self.config.api_key.clone().unwrap_or_default();
                        self.inputs = vec![Input::new(existing)];
                        self.focused_input = 0;
                        self.sub_stage = SubStage::Provider(ProviderStage::Key);
                    }
                }
                KeyCode::Esc => {
                    self.sub_stage = SubStage::Provider(ProviderStage::Select);
                }
                _ => {}
            },
            SubStage::Provider(ProviderStage::Key) => {
                if let Some(flow) = self.handle_form_input(key, 1) {
                    return flow;
                }
                if key.code == KeyCode::Enter {
                    let key_val = self.inputs[0].value().to_string();
                    self.config.api_key = Some(key_val.clone());
                    let (id, _, _, default_model) = self.providers[self.selected_provider_idx];
                    // Seed the provider config so model selection and final save
                    // know which provider the user actually picked.
                    self.config.provider = Some(ProviderConfig {
                        name: id.to_string(),
                        default_model: default_model.to_string(),
                        base_url: None,
                        model: None,
                    });
                    match id {
                        "gemini" => self.spawn_fetch(FetchKind::Gemini(key_val)),
                        "openai" => self.spawn_fetch(FetchKind::OpenAi(key_val)),
                        "anthropic" => self.spawn_fetch(FetchKind::Anthropic(key_val)),
                        "openrouter" => self.spawn_fetch(FetchKind::OpenRouter(key_val)),
                        "opencode" => self.spawn_fetch(FetchKind::OpenCode(key_val)),
                        "kilocode" => self.spawn_fetch(FetchKind::Kilocode(key_val)),
                        _ => {}
                    }
                    self.sub_stage = SubStage::Provider(ProviderStage::Fetching);
                } else if key.code == KeyCode::Esc {
                    self.sub_stage = SubStage::Provider(ProviderStage::Select);
                }
            }
            SubStage::Provider(ProviderStage::BaseUrl) => {
                if let Some(flow) = self.handle_form_input(key, 2) {
                    return flow;
                }
                if key.code == KeyCode::Enter {
                    let url = self.inputs[0].value().to_string();
                    let key_val = if self.inputs.len() > 1 {
                        self.inputs[1].value().to_string()
                    } else {
                        String::new()
                    };
                    if self.focused_input == 0 && self.inputs.len() == 1 {
                        // After base url, ask for key
                        self.inputs.push(Input::new(key_val));
                        self.focused_input = 1;
                        return std::ops::ControlFlow::Continue(());
                    }
                    self.config.api_key = Some(key_val.clone());
                    self.config.custom_base_url = Some(Some(url.clone()));
                    self.config.provider = Some(ProviderConfig {
                        name: "openai-compatible".to_string(),
                        default_model: "local-model".to_string(),
                        base_url: Some(url.clone()),
                        model: None,
                    });
                    self.spawn_fetch(FetchKind::Custom(url, key_val));
                    self.sub_stage = SubStage::Provider(ProviderStage::Fetching);
                } else if key.code == KeyCode::Esc {
                    self.sub_stage = SubStage::Provider(ProviderStage::Select);
                }
            }
            SubStage::Provider(ProviderStage::Fetching) => {
                // handled by on_tick + top-level fetching check
            }
            SubStage::Provider(ProviderStage::ModelSelect) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.list_up(),
                KeyCode::Down | KeyCode::Char('j') => self.list_down(),
                KeyCode::Enter => {
                    let idx = self.list_state.selected().unwrap_or(0);
                    let model = self.model_ids.get(idx).cloned().unwrap_or_default();
                    if let Some(p) = &mut self.config.provider {
                        p.default_model = model.clone();
                        p.model = Some(model.clone());
                        self.config.selected_default_model = Some(model);
                    }
                    self.advance_step();
                }
                KeyCode::Esc => {
                    self.sub_stage = SubStage::Provider(ProviderStage::Select);
                }
                _ => {}
            },
            _ => {}
        }
        std::ops::ControlFlow::Continue(())
    }

    fn handle_cheap_model(&mut self, key: KeyEvent) -> std::ops::ControlFlow<()> {
        if self.fetching {
            if key.code == KeyCode::Esc {
                self.fetching = false;
                self.fetch_rx = None;
                self.sub_stage = SubStage::Cheap(CheapStage::Select);
            }
            return std::ops::ControlFlow::Continue(());
        }

        match self.sub_stage {
            SubStage::Cheap(CheapStage::Select) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.list_up(),
                KeyCode::Down | KeyCode::Char('j') => self.list_down(),
                KeyCode::Enter => {
                    self.selected_cheap_provider_idx =
                        self.list_state.selected().unwrap_or(0);
                    let (id, _) = self.cheap_providers[self.selected_cheap_provider_idx];
                    if id == "none" {
                        self.config.cheap_provider = Some(None);
                        self.config.cheap_model = Some(None);
                        self.config.cheap_api_key = Some(None);
                        self.advance_step();
                    } else {
                        let existing = self
                            .config
                            .cheap_api_key
                            .as_ref()
                            .and_then(|k| k.as_ref())
                            .cloned()
                            .unwrap_or_default();
                        self.inputs = vec![Input::new(existing)];
                        self.focused_input = 0;
                        self.sub_stage = SubStage::Cheap(CheapStage::Key);
                    }
                }
                KeyCode::Esc => self.go_back(),
                _ => {}
            },
            SubStage::Cheap(CheapStage::Key) => {
                if let Some(flow) = self.handle_form_input(key, 1) {
                    return flow;
                }
                if key.code == KeyCode::Enter {
                    let key_val = self.inputs[0].value().to_string();
                    let (id, _) = self.cheap_providers[self.selected_cheap_provider_idx];
                    self.config.cheap_provider = Some(Some(id.to_string()));
                    self.config.cheap_api_key = Some(Some(key_val.clone()));
                    match id {
                        "openrouter" => self.spawn_fetch(FetchKind::CheapOpenRouter(key_val)),
                        "opencode" => self.spawn_fetch(FetchKind::CheapOpenCode(key_val)),
                        "kilocode" => self.spawn_fetch(FetchKind::CheapKilocode(key_val)),
                        _ => {}
                    }
                    self.sub_stage = SubStage::Cheap(CheapStage::Fetching);
                } else if key.code == KeyCode::Esc {
                    self.sub_stage = SubStage::Cheap(CheapStage::Select);
                }
            }
            SubStage::Cheap(CheapStage::Fetching) => {}
            SubStage::Cheap(CheapStage::ModelSelect) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.list_up(),
                KeyCode::Down | KeyCode::Char('j') => self.list_down(),
                KeyCode::Enter => {
                    let idx = self.list_state.selected().unwrap_or(0);
                    let model = self.model_ids.get(idx).cloned().unwrap_or_default();
                    self.config.cheap_model = Some(Some(model));
                    self.advance_step();
                }
                KeyCode::Esc => {
                    self.sub_stage = SubStage::Cheap(CheapStage::Select);
                }
                _ => {}
            },
            _ => {}
        }
        std::ops::ControlFlow::Continue(())
    }

    fn handle_timezone(&mut self, key: KeyEvent) -> std::ops::ControlFlow<()> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.list_up(),
            KeyCode::Down | KeyCode::Char('j') => self.list_down(),
            KeyCode::Enter => {
                let idx = self.list_state.selected().unwrap_or(0);
                let tz = COMMON_TIMEZONES[idx].0.to_string();
                self.config.timezone = Some(tz);
                self.advance_step();
            }
            KeyCode::Esc => self.go_back(),
            _ => {}
        }
        std::ops::ControlFlow::Continue(())
    }

    fn handle_memory(&mut self, key: KeyEvent) -> std::ops::ControlFlow<()> {
        match self.sub_stage {
            SubStage::Memory(MemoryStage::SelectBackend) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.list_up(),
                KeyCode::Down | KeyCode::Char('j') => self.list_down(),
                KeyCode::Enter => {
                    let idx = self.list_state.selected().unwrap_or(0);
                    let backend = match idx {
                        1 => "markdown",
                        2 => "none",
                        _ => "sqlite",
                    };
                    self.config.memory_backend = Some(backend.to_string());
                    if backend == "sqlite" {
                        self.sub_stage = SubStage::Memory(MemoryStage::ConfirmEmbed);
                        self.list_state.select(Some(0));
                    } else {
                        self.advance_step();
                    }
                }
                KeyCode::Esc => self.go_back(),
                _ => {}
            },
            SubStage::Memory(MemoryStage::ConfirmEmbed) => match key.code {
                KeyCode::Up | KeyCode::Down | KeyCode::Char('k') | KeyCode::Char('j') => {
                    let sel = self.list_state.selected().unwrap_or(0);
                    self.list_state
                        .select(Some(if sel == 0 { 1 } else { 0 }));
                }
                KeyCode::Enter => {
                    if self.list_state.selected().unwrap_or(0) == 0 {
                        // yes
                        let existing = self
                            .config
                            .embed_key
                            .as_ref()
                            .and_then(|k| k.as_ref())
                            .cloned()
                            .unwrap_or_default();
                        self.inputs = vec![Input::new(existing)];
                        self.focused_input = 0;
                        self.sub_stage = SubStage::Memory(MemoryStage::EmbedKey);
                    } else {
                        self.advance_step();
                    }
                }
                KeyCode::Esc => {
                    self.sub_stage = SubStage::Memory(MemoryStage::SelectBackend);
                }
                _ => {}
            },
            SubStage::Memory(MemoryStage::EmbedKey) => {
                if let Some(flow) = self.handle_form_input(key, 1) {
                    return flow;
                }
                if key.code == KeyCode::Enter {
                    let key_val = self.inputs[0].value().to_string();
                    self.config.embed_provider = Some(Some("huggingface".to_string()));
                    self.config.embed_model =
                        Some(Some("Qwen/Qwen3-Embedding-0.6B".to_string()));
                    self.config.embed_key = Some(Some(key_val));
                    self.advance_step();
                } else if key.code == KeyCode::Esc {
                    self.sub_stage = SubStage::Memory(MemoryStage::ConfirmEmbed);
                }
            }
            _ => {}
        }
        std::ops::ControlFlow::Continue(())
    }

    fn handle_voice(&mut self, key: KeyEvent) -> std::ops::ControlFlow<()> {
        match self.sub_stage {
            SubStage::Voice(VoiceStage::Confirm) => match key.code {
                KeyCode::Up | KeyCode::Down | KeyCode::Char('k') | KeyCode::Char('j') => {
                    let sel = self.list_state.selected().unwrap_or(0);
                    self.list_state
                        .select(Some(if sel == 0 { 1 } else { 0 }));
                }
                KeyCode::Enter => {
                    if self.list_state.selected().unwrap_or(0) == 0 {
                        let existing = self.config.groq_key.clone().unwrap_or_default();
                        self.inputs = vec![Input::new(existing)];
                        self.focused_input = 0;
                        self.sub_stage = SubStage::Voice(VoiceStage::Key);
                    } else {
                        self.config.groq_key = None;
                        self.advance_step();
                    }
                }
                KeyCode::Esc => self.go_back(),
                _ => {}
            },
            SubStage::Voice(VoiceStage::Key) => {
                if let Some(flow) = self.handle_form_input(key, 1) {
                    return flow;
                }
                if key.code == KeyCode::Enter {
                    let key_val = self.inputs[0].value().to_string();
                    self.config.groq_key = Some(key_val);
                    self.advance_step();
                } else if key.code == KeyCode::Esc {
                    self.sub_stage = SubStage::Voice(VoiceStage::Confirm);
                }
            }
            _ => {}
        }
        std::ops::ControlFlow::Continue(())
    }

    fn handle_channels(&mut self, key: KeyEvent) -> std::ops::ControlFlow<()> {
        match self.sub_stage {
            SubStage::Channel(ChannelStage::Select) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.list_up(),
                KeyCode::Down | KeyCode::Char('j') => self.list_down(),
                KeyCode::Enter => {
                    let idx = self.list_state.selected().unwrap_or(0);
                    match idx {
                        1 => {
                            // Telegram
                            let (tok, user) = self
                                .config
                                .telegram
                                .as_ref()
                                .and_then(|t| t.as_ref())
                                .map(|(a, b)| (a.clone(), b.clone()))
                                .unwrap_or_default();
                            self.inputs = vec![Input::new(tok), Input::new(user)];
                            self.focused_input = 0;
                            self.sub_stage = SubStage::Channel(ChannelStage::Telegram);
                        }
                        2 => {
                            // WhatsApp
                            let phone = self
                                .config
                                .whatsapp_native
                                .as_ref()
                                .and_then(|w| w.as_ref())
                                .map(|(_, p)| p.clone())
                                .unwrap_or_else(|| "+1".to_string());
                            self.inputs = vec![Input::new(phone)];
                            self.focused_input = 0;
                            self.sub_stage = SubStage::Channel(ChannelStage::Whatsapp);
                        }
                        3 => {
                            // Email
                            let e = self
                                .config
                                .email
                                .as_ref()
                                .and_then(|e| e.as_ref())
                                .cloned();
                            let (em, pa, sh, sp, ih, ip) = match e {
                                Some((a, b, c, d, e, f)) => (
                                    a, b, c,
                                    d.to_string(), e,
                                    f.to_string(),
                                ),
                                None => (
                                    String::new(),
                                    String::new(),
                                    "smtp.gmail.com".to_string(),
                                    "587".to_string(),
                                    "imap.gmail.com".to_string(),
                                    "993".to_string(),
                                ),
                            };
                            self.inputs = vec![
                                Input::new(em),
                                Input::new(pa),
                                Input::new(sh),
                                Input::new(sp),
                                Input::new(ih),
                                Input::new(ip),
                            ];
                            self.focused_input = 0;
                            self.sub_stage = SubStage::Channel(ChannelStage::Email);
                        }
                        4 => {
                            // Both (Telegram + WhatsApp)
                            let (tok, user) = self
                                .config
                                .telegram
                                .as_ref()
                                .and_then(|t| t.as_ref())
                                .map(|(a, b)| (a.clone(), b.clone()))
                                .unwrap_or_default();
                            let phone = self
                                .config
                                .whatsapp_native
                                .as_ref()
                                .and_then(|w| w.as_ref())
                                .map(|(_, p)| p.clone())
                                .unwrap_or_else(|| "+1".to_string());
                            self.inputs = vec![
                                Input::new(tok),
                                Input::new(user),
                                Input::new(phone),
                            ];
                            self.focused_input = 0;
                            // We'll use a virtual "Both" stage reusing Telegram widget for now
                            // but actually we need a new stage. Let's hack it by storing in Telegram
                            // and then prompting for WhatsApp after.
                            // To keep it simple, "Both" will just configure Telegram first, then WhatsApp.
                            self.sub_stage = SubStage::Channel(ChannelStage::Telegram);
                        }
                        _ => {
                            self.config.telegram = Some(None);
                            self.config.whatsapp_native = Some(None);
                            self.config.email = Some(None);
                            self.advance_step();
                        }
                    }
                }
                KeyCode::Esc => self.go_back(),
                _ => {}
            },
            SubStage::Channel(ChannelStage::Telegram) => {
                if let Some(flow) = self.handle_form_input(key, 2) {
                    return flow;
                }
                if key.code == KeyCode::Enter {
                    let token = self.inputs[0].value().to_string();
                    let user = self.inputs[1].value().to_string();
                    self.config.telegram = Some(Some((token, user)));
                    // If user selected "Both", we need to go to Whatsapp next.
                    // We need to know if original choice was "Both". Let's check list_state selected before we changed stage?
                    // Actually, we can infer: if email is None AND telegram is being set and whatsapp was previously None... it's messy.
                    // Simpler: "Both" is not supported in this TUI version, or we treat "Both" as Telegram + prompt for WhatsApp.
                    // Let's use a trick: if inputs.len() == 3, it was "Both".
                    if self.inputs.len() == 3 {
                        let phone = self.inputs[2].value().to_string();
                        self.config.whatsapp_native =
                            Some(Some(("http://localhost:18790".to_string(), phone)));
                        self.advance_step();
                    } else {
                        self.advance_step();
                    }
                } else if key.code == KeyCode::Esc {
                    self.sub_stage = SubStage::Channel(ChannelStage::Select);
                }
            }
            SubStage::Channel(ChannelStage::Whatsapp) => {
                if let Some(flow) = self.handle_form_input(key, 1) {
                    return flow;
                }
                if key.code == KeyCode::Enter {
                    let phone = self.inputs[0].value().to_string();
                    self.config.whatsapp_native =
                        Some(Some(("http://localhost:18790".to_string(), phone)));
                    self.advance_step();
                } else if key.code == KeyCode::Esc {
                    self.sub_stage = SubStage::Channel(ChannelStage::Select);
                }
            }
            SubStage::Channel(ChannelStage::Email) => {
                if let Some(flow) = self.handle_form_input(key, 6) {
                    return flow;
                }
                if key.code == KeyCode::Enter {
                    let email = self.inputs[0].value().to_string();
                    let pass = self.inputs[1].value().to_string();
                    let smtp_host = self.inputs[2].value().to_string();
                    let smtp_port: u16 = self.inputs[3].value().parse().unwrap_or(587);
                    let imap_host = self.inputs[4].value().to_string();
                    let imap_port: u16 = self.inputs[5].value().parse().unwrap_or(993);
                    self.config.email = Some(Some((
                        email, pass, smtp_host, smtp_port, imap_host, imap_port,
                    )));
                    self.advance_step();
                } else if key.code == KeyCode::Esc {
                    self.sub_stage = SubStage::Channel(ChannelStage::Select);
                }
            }
            _ => {}
        }
        std::ops::ControlFlow::Continue(())
    }

    fn handle_composio(&mut self, key: KeyEvent) -> std::ops::ControlFlow<()> {
        match self.sub_stage {
            SubStage::Composio(ComposioStage::Confirm) => match key.code {
                KeyCode::Up | KeyCode::Down | KeyCode::Char('k') | KeyCode::Char('j') => {
                    let sel = self.list_state.selected().unwrap_or(0);
                    self.list_state
                        .select(Some(if sel == 0 { 1 } else { 0 }));
                }
                KeyCode::Enter => {
                    if self.list_state.selected().unwrap_or(0) == 0 {
                        let key = self
                            .config
                            .composio_api_key
                            .as_ref()
                            .and_then(|k| k.as_ref())
                            .cloned()
                            .unwrap_or_default();
                        let ent = self
                            .config
                            .composio_entity_id
                            .clone()
                            .unwrap_or_else(|| "default".to_string());
                        self.inputs = vec![Input::new(key), Input::new(ent)];
                        self.focused_input = 0;
                        self.sub_stage = SubStage::Composio(ComposioStage::Details);
                    } else {
                        self.config.composio_enabled = Some(false);
                        self.advance_step();
                    }
                }
                KeyCode::Esc => self.go_back(),
                _ => {}
            },
            SubStage::Composio(ComposioStage::Details) => {
                if let Some(flow) = self.handle_form_input(key, 2) {
                    return flow;
                }
                if key.code == KeyCode::Enter {
                    let key_val = self.inputs[0].value().to_string();
                    let ent = self.inputs[1].value().to_string();
                    self.config.composio_enabled = Some(true);
                    self.config.composio_api_key = Some(Some(key_val));
                    self.config.composio_entity_id = Some(ent);
                    self.advance_step();
                } else if key.code == KeyCode::Esc {
                    self.sub_stage = SubStage::Composio(ComposioStage::Confirm);
                }
            }
            _ => {}
        }
        std::ops::ControlFlow::Continue(())
    }

    fn handle_websearch(&mut self, key: KeyEvent) -> std::ops::ControlFlow<()> {
        match self.sub_stage {
            SubStage::WebSearch(WebSearchStage::Select) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.list_up(),
                KeyCode::Down | KeyCode::Char('j') => self.list_down(),
                KeyCode::Enter => {
                    let idx = self.list_state.selected().unwrap_or(0);
                    match idx {
                        0 => {
                            self.config.search_provider = Some("gemini_cli".to_string());
                            if which::which("gemini").is_ok() {
                                self.config.brave_api_key = Some(None);
                                self.advance_step();
                            } else {
                                self.sub_stage = SubStage::WebSearch(WebSearchStage::GeminiCheck);
                            }
                        }
                        1 => {
                            self.config.search_provider = Some("brave".to_string());
                            let existing = self
                                .config
                                .brave_api_key
                                .as_ref()
                                .and_then(|k| k.as_ref())
                                .cloned()
                                .unwrap_or_default();
                            self.inputs = vec![Input::new(existing)];
                            self.focused_input = 0;
                            self.sub_stage = SubStage::WebSearch(WebSearchStage::BraveKey);
                        }
                        2 => {
                            self.config.search_provider = Some("duckduckgo".to_string());
                            self.config.brave_api_key = Some(None);
                            self.advance_step();
                        }
                        _ => {
                            self.config.search_provider = Some("duckduckgo".to_string());
                            self.config.brave_api_key = Some(None);
                            self.advance_step();
                        }
                    }
                }
                KeyCode::Esc => self.go_back(),
                _ => {}
            },
            SubStage::WebSearch(WebSearchStage::BraveKey) => {
                if let Some(flow) = self.handle_form_input(key, 1) {
                    return flow;
                }
                if key.code == KeyCode::Enter {
                    let key_val = self.inputs[0].value().to_string();
                    self.config.brave_api_key = Some(Some(key_val));
                    self.advance_step();
                } else if key.code == KeyCode::Esc {
                    self.sub_stage = SubStage::WebSearch(WebSearchStage::Select);
                }
            }
            SubStage::WebSearch(WebSearchStage::GeminiCheck) => match key.code {
                KeyCode::Enter | KeyCode::Esc => {
                    self.config.brave_api_key = Some(None);
                    self.advance_step();
                }
                _ => {}
            },
            _ => {}
        }
        std::ops::ControlFlow::Continue(())
    }

    fn handle_pushover(&mut self, key: KeyEvent) -> std::ops::ControlFlow<()> {
        match self.sub_stage {
            SubStage::Pushover(PushoverStage::Confirm) => match key.code {
                KeyCode::Up | KeyCode::Down | KeyCode::Char('k') | KeyCode::Char('j') => {
                    let sel = self.list_state.selected().unwrap_or(0);
                    self.list_state
                        .select(Some(if sel == 0 { 1 } else { 0 }));
                }
                KeyCode::Enter => {
                    if self.list_state.selected().unwrap_or(0) == 0 {
                        let (t, u) = self
                            .config
                            .pushover
                            .as_ref()
                            .and_then(|p| p.as_ref())
                            .map(|(a, b)| (a.clone(), b.clone()))
                            .unwrap_or_default();
                        self.inputs = vec![Input::new(t), Input::new(u)];
                        self.focused_input = 0;
                        self.sub_stage = SubStage::Pushover(PushoverStage::Details);
                    } else {
                        self.config.pushover = Some(None);
                        self.advance_step();
                    }
                }
                KeyCode::Esc => self.go_back(),
                _ => {}
            },
            SubStage::Pushover(PushoverStage::Details) => {
                if let Some(flow) = self.handle_form_input(key, 2) {
                    return flow;
                }
                if key.code == KeyCode::Enter {
                    let token = self.inputs[0].value().to_string();
                    let user = self.inputs[1].value().to_string();
                    self.config.pushover = Some(Some((token, user)));
                    self.advance_step();
                } else if key.code == KeyCode::Esc {
                    self.sub_stage = SubStage::Pushover(PushoverStage::Confirm);
                }
            }
            _ => {}
        }
        std::ops::ControlFlow::Continue(())
    }

    fn handle_summary(&mut self, key: KeyEvent) -> std::ops::ControlFlow<()> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                let sel = self.summary_list.selected().unwrap_or(0);
                if sel > 0 {
                    self.summary_list.select(Some(sel - 1));
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let sel = self.summary_list.selected().unwrap_or(0);
                let max = if self.is_edit { 10 } else { 2 };
                if sel < max {
                    self.summary_list.select(Some(sel + 1));
                }
            }
            KeyCode::Enter => {
                let sel = self.summary_list.selected().unwrap_or(0);
                if self.is_edit {
                    match sel {
                        0 => {
                            self.current_step = Step::Provider;
                            self.sub_stage = SubStage::Provider(ProviderStage::Select);
                            self.init_selections();
                        }
                        1 => {
                            self.current_step = Step::CheapModel;
                            self.sub_stage = SubStage::Cheap(CheapStage::Select);
                            self.list_state.select(Some(self.selected_cheap_provider_idx));
                        }
                        2 => {
                            self.current_step = Step::Timezone;
                            let tz = self.config.timezone.as_deref().unwrap_or("UTC");
                            let idx = COMMON_TIMEZONES
                                .iter()
                                .position(|(t, _)| *t == tz)
                                .unwrap_or(0);
                            self.list_state.select(Some(idx));
                        }
                        3 => {
                            self.current_step = Step::Memory;
                            self.sub_stage = SubStage::Memory(MemoryStage::SelectBackend);
                            let idx = match self.config.memory_backend.as_deref() {
                                Some("markdown") => 1,
                                Some("none") => 2,
                                _ => 0,
                            };
                            self.list_state.select(Some(idx));
                        }
                        4 => {
                            self.current_step = Step::Voice;
                            self.sub_stage = SubStage::Voice(VoiceStage::Confirm);
                            let has = self.config.groq_key.is_some();
                            self.list_state.select(Some(if has { 0 } else { 1 }));
                        }
                        5 => {
                            self.current_step = Step::Channels;
                            self.sub_stage = SubStage::Channel(ChannelStage::Select);
                            let has_tg = self
                                .config
                                .telegram
                                .as_ref()
                                .and_then(|t| t.as_ref())
                                .is_some();
                            let has_wa = self
                                .config
                                .whatsapp_native
                                .as_ref()
                                .and_then(|w| w.as_ref())
                                .is_some();
                            let has_em = self
                                .config
                                .email
                                .as_ref()
                                .and_then(|e| e.as_ref())
                                .is_some();
                            let idx = match (has_tg, has_wa, has_em) {
                                (true, false, false) => 1,
                                (false, true, false) => 2,
                                (false, false, true) => 3,
                                (true, true, false) => 4,
                                _ => 0,
                            };
                            self.list_state.select(Some(idx));
                        }
                        6 => {
                            self.current_step = Step::Composio;
                            self.sub_stage = SubStage::Composio(ComposioStage::Confirm);
                            let has = self.config.composio_enabled.unwrap_or(false);
                            self.list_state.select(Some(if has { 0 } else { 1 }));
                        }
                        7 => {
                            self.current_step = Step::WebSearch;
                            self.sub_stage = SubStage::WebSearch(WebSearchStage::Select);
                            let idx = match self.config.search_provider.as_deref() {
                                Some("brave") => 1,
                                Some("duckduckgo") => 2,
                                Some("none") => 3,
                                _ => 0,
                            };
                            self.list_state.select(Some(idx));
                        }
                        8 => {
                            self.current_step = Step::Pushover;
                            self.sub_stage = SubStage::Pushover(PushoverStage::Confirm);
                            let has = self.config.pushover.as_ref().and_then(|p| p.as_ref()).is_some();
                            self.list_state.select(Some(if has { 0 } else { 1 }));
                        }
                        9 => {
                            self.should_save = true;
                            self.current_step = Step::Done;
                        }
                        _ => {
                            self.should_quit = true;
                        }
                    }
                } else {
                    match sel {
                        0 => {
                            self.should_save = true;
                            self.current_step = Step::Done;
                        }
                        1 => {
                            self.current_step = Step::Provider;
                            self.sub_stage = SubStage::Provider(ProviderStage::Select);
                            self.init_selections();
                        }
                        _ => {
                            self.should_quit = true;
                        }
                    }
                }
            }
            KeyCode::Esc => {
                if self.is_edit {
                    self.should_quit = true;
                } else {
                    self.current_step = Step::Pushover;
                    self.sub_stage = SubStage::Pushover(PushoverStage::Confirm);
                    let has = self.config.pushover.as_ref().and_then(|p| p.as_ref()).is_some();
                    self.list_state.select(Some(if has { 0 } else { 1 }));
                }
            }
            _ => {}
        }
        std::ops::ControlFlow::Continue(())
    }

    fn handle_done(&mut self, key: KeyEvent) -> std::ops::ControlFlow<()> {
        if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q')) {
            self.should_quit = true;
        }
        std::ops::ControlFlow::Continue(())
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Navigation helpers
    // ═════════════════════════════════════════════════════════════════════════

    fn advance_step(&mut self) {
        let steps = Step::all();
        let current_idx = steps.iter().position(|s| *s == self.current_step);
        if let Some(idx) = current_idx {
            if idx + 1 < steps.len() {
                self.current_step = steps[idx + 1];
                self.sub_stage = match self.current_step {
                    Step::Provider => SubStage::Provider(ProviderStage::Select),
                    Step::CheapModel => SubStage::Cheap(CheapStage::Select),
                    Step::Memory => SubStage::Memory(MemoryStage::SelectBackend),
                    Step::Voice => SubStage::Voice(VoiceStage::Confirm),
                    Step::Channels => SubStage::Channel(ChannelStage::Select),
                    Step::Composio => SubStage::Composio(ComposioStage::Confirm),
                    Step::WebSearch => SubStage::WebSearch(WebSearchStage::Select),
                    Step::Pushover => SubStage::Pushover(PushoverStage::Confirm),
                    _ => SubStage::None,
                };
                self.inputs.clear();
                self.focused_input = 0;
                self.init_selections_for_current_step();
            } else {
                self.current_step = Step::Summary;
                self.sub_stage = SubStage::None;
                self.summary_list.select(Some(0));
            }
        }
    }

    fn go_back(&mut self) {
        let steps = Step::all();
        let current_idx = steps.iter().position(|s| *s == self.current_step);
        if let Some(idx) = current_idx {
            if idx > 0 {
                self.current_step = steps[idx - 1];
                self.sub_stage = match self.current_step {
                    Step::Provider => SubStage::Provider(ProviderStage::Select),
                    Step::CheapModel => SubStage::Cheap(CheapStage::Select),
                    Step::Memory => SubStage::Memory(MemoryStage::SelectBackend),
                    Step::Voice => SubStage::Voice(VoiceStage::Confirm),
                    Step::Channels => SubStage::Channel(ChannelStage::Select),
                    Step::Composio => SubStage::Composio(ComposioStage::Confirm),
                    Step::WebSearch => SubStage::WebSearch(WebSearchStage::Select),
                    Step::Pushover => SubStage::Pushover(PushoverStage::Confirm),
                    _ => SubStage::None,
                };
                self.inputs.clear();
                self.focused_input = 0;
                self.init_selections_for_current_step();
            } else {
                self.current_step = Step::Welcome;
                self.sub_stage = SubStage::None;
            }
        }
    }

    fn init_selections_for_current_step(&mut self) {
        match self.current_step {
            Step::Provider => {
                self.init_selections();
                self.sub_stage = SubStage::Provider(ProviderStage::Select);
            }
            Step::CheapModel => {
                self.list_state.select(Some(self.selected_cheap_provider_idx));
                self.sub_stage = SubStage::Cheap(CheapStage::Select);
            }
            Step::Timezone => {
                let tz = self.config.timezone.as_deref().unwrap_or("UTC");
                let idx = COMMON_TIMEZONES
                    .iter()
                    .position(|(t, _)| *t == tz)
                    .unwrap_or(0);
                self.list_state.select(Some(idx));
            }
            Step::Memory => {
                let idx = match self.config.memory_backend.as_deref() {
                    Some("markdown") => 1,
                    Some("none") => 2,
                    _ => 0,
                };
                self.list_state.select(Some(idx));
            }
            Step::Voice => {
                let has = self.config.groq_key.is_some();
                self.list_state.select(Some(if has { 0 } else { 1 }));
            }
            Step::Channels => {
                let has_tg = self
                    .config
                    .telegram
                    .as_ref()
                    .and_then(|t| t.as_ref())
                    .is_some();
                let has_wa = self
                    .config
                    .whatsapp_native
                    .as_ref()
                    .and_then(|w| w.as_ref())
                    .is_some();
                let has_em = self
                    .config
                    .email
                    .as_ref()
                    .and_then(|e| e.as_ref())
                    .is_some();
                let idx = match (has_tg, has_wa, has_em) {
                    (true, false, false) => 1,
                    (false, true, false) => 2,
                    (false, false, true) => 3,
                    (true, true, false) => 4,
                    _ => 0,
                };
                self.list_state.select(Some(idx));
            }
            Step::Composio => {
                let has = self.config.composio_enabled.unwrap_or(false);
                self.list_state.select(Some(if has { 0 } else { 1 }));
            }
            Step::WebSearch => {
                let idx = match self.config.search_provider.as_deref() {
                    Some("brave") => 1,
                    Some("duckduckgo") => 2,
                    Some("none") => 3,
                    _ => 0,
                };
                self.list_state.select(Some(idx));
            }
            Step::Pushover => {
                let has = self.config.pushover.as_ref().and_then(|p| p.as_ref()).is_some();
                self.list_state.select(Some(if has { 0 } else { 1 }));
            }
            _ => {}
        }
    }

    fn list_up(&mut self) {
        let sel = self.list_state.selected().unwrap_or(0);
        if sel > 0 {
            self.list_state.select(Some(sel - 1));
        }
    }

    fn list_down(&mut self) {
        let sel = self.list_state.selected().unwrap_or(0);
        // We don't know the exact len here easily without passing it around.
        // We'll rely on the draw/handle methods to clamp or use large enough lists.
        self.list_state.select(Some(sel + 1));
    }

    fn handle_form_input(
        &mut self,
        key: KeyEvent,
        max_inputs: usize,
    ) -> Option<std::ops::ControlFlow<()>> {
        match key.code {
            KeyCode::Tab => {
                self.focused_input = (self.focused_input + 1) % max_inputs;
                return Some(std::ops::ControlFlow::Continue(()));
            }
            KeyCode::BackTab => {
                self.focused_input = (self.focused_input + max_inputs - 1) % max_inputs;
                return Some(std::ops::ControlFlow::Continue(()));
            }
            KeyCode::Esc | KeyCode::Enter => {
                // Let caller handle
                return None;
            }
            _ => {
                if self.focused_input < self.inputs.len() {
                    self.inputs[self.focused_input].handle_event(&Event::Key(key));
                }
                return Some(std::ops::ControlFlow::Continue(()));
            }
        }
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Fetching
    // ═════════════════════════════════════════════════════════════════════════

    fn spawn_fetch(&mut self, kind: FetchKind) {
        self.fetching = true;
        let (tx, rx) = bounded(1);
        self.fetch_rx = Some(rx);

        std::thread::spawn(move || {
            let result = match kind {
                FetchKind::Gemini(key) => {
                    if key == "cli_oauth" {
                        fetch_gemini_models("dummy").map(FetchResult::Strings)
                    } else {
                        fetch_gemini_models(&key).map(FetchResult::Strings)
                    }
                }
                FetchKind::OpenAi(key) => fetch_openai_models(&key).map(FetchResult::Strings),
                FetchKind::Anthropic(key) => {
                    fetch_anthropic_models(&key).map(FetchResult::Strings)
                }
                FetchKind::OpenRouter(key) => fetch_openrouter_free_models(&key)
                    .map(FetchResult::OpenRouterModels),
                FetchKind::OpenCode(key) => {
                    fetch_openai_compat_models("https://opencode.ai/zen/v1", &key)
                        .map(FetchResult::Strings)
                }
                FetchKind::Kilocode(key) => {
                    fetch_kilocode_free_models(&key).map(FetchResult::Strings)
                }
                FetchKind::Ollama => fetch_ollama_models().map(FetchResult::Strings),
                FetchKind::LmStudio => {
                    fetch_ollama_compat_models("http://localhost:1234/v1").map(FetchResult::Strings)
                }
                FetchKind::Custom(url, key) => {
                    fetch_openai_compat_models(&url, &key).map(FetchResult::Strings)
                }
                FetchKind::CheapOpenRouter(key) => fetch_openrouter_free_models(&key)
                    .map(FetchResult::OpenRouterModels),
                FetchKind::CheapOpenCode(key) => {
                    fetch_openai_compat_models("https://opencode.ai/zen/v1", &key)
                        .map(FetchResult::Strings)
                }
                FetchKind::CheapKilocode(key) => {
                    fetch_kilocode_free_models(&key).map(FetchResult::Strings)
                }
            };
            let msg = match result {
                Ok(v) => v,
                Err(e) => FetchResult::Error(e.to_string()),
            };
            let _ = tx.send(msg);
        });
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Drawing
    // ═════════════════════════════════════════════════════════════════════════

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let main_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_header(f, main_layout[0]);
        self.draw_body(f, main_layout[1]);
        self.draw_footer(f, main_layout[2]);

        if let Some(ref toast) = self.toast {
            self.draw_toast(f, toast);
        }

        if self.show_help {
            self.draw_help(f);
        }

        // Set cursor for input fields
        if !self.inputs.is_empty() && self.focused_input < self.inputs.len() {
            // The actual cursor positioning is done inside draw methods
        }
    }

    fn draw_header(&self, f: &mut Frame, area: Rect) {
        let title = if self.is_edit {
            "OpenPaw · Edit Configuration"
        } else {
            "OpenPaw · Setup Wizard"
        };
        let block = Block::default()
            .title(
                Span::styled(title, Style::default().fg(THEME.primary).add_modifier(Modifier::BOLD)),
            )
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(THEME.border));
        f.render_widget(block, area);
    }

    fn draw_body(&mut self, f: &mut Frame, area: Rect) {
        let body_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(30), Constraint::Min(20)])
            .split(area);

        self.draw_sidebar(f, body_layout[0]);

        match self.current_step {
            Step::Welcome => self.draw_welcome(f, body_layout[1]),
            Step::Provider => self.draw_provider(f, body_layout[1]),
            Step::CheapModel => self.draw_cheap_model(f, body_layout[1]),
            Step::Timezone => self.draw_timezone(f, body_layout[1]),
            Step::Memory => self.draw_memory(f, body_layout[1]),
            Step::Voice => self.draw_voice(f, body_layout[1]),
            Step::Channels => self.draw_channels(f, body_layout[1]),
            Step::Composio => self.draw_composio(f, body_layout[1]),
            Step::WebSearch => self.draw_websearch(f, body_layout[1]),
            Step::Pushover => self.draw_pushover(f, body_layout[1]),
            Step::Summary => self.draw_summary(f, body_layout[1]),
            Step::Done => self.draw_done(f, body_layout[1]),
        }
    }

    fn draw_sidebar(&mut self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(Span::styled("Steps", Style::default().fg(THEME.muted)))
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(THEME.border));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let steps = Step::all();
        let items: Vec<ListItem> = steps
            .iter()
            .map(|step| {
                let label = step.label();
                let is_done = *step < self.current_step;
                let is_current = *step == self.current_step;
                let style = if is_current {
                    Style::default()
                        .fg(THEME.highlight_fg)
                        .bg(THEME.highlight_bg)
                        .add_modifier(Modifier::BOLD)
                } else if is_done {
                    Style::default().fg(THEME.success)
                } else {
                    Style::default().fg(THEME.muted)
                };
                let prefix = if is_done {
                    "  ✓ "
                } else if is_current {
                    "  ▸ "
                } else {
                    "    "
                };
                ListItem::new(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(label, style),
                ]))
            })
            .collect();

        let list = List::new(items).highlight_style(Style::default());
        f.render_widget(list, inner);
    }

    fn draw_footer(&self, f: &mut Frame, area: Rect) {
        let help_text = if self.show_help {
            "Press Esc or ? to close help"
        } else {
            "? Help  ↑↓ Navigate  Enter Select  Esc Back  Ctrl+C Quit"
        };
        let paragraph = Paragraph::new(help_text)
            .style(Style::default().fg(THEME.muted))
            .alignment(Alignment::Center);
        f.render_widget(paragraph, area);
    }

    fn draw_toast(&self, f: &mut Frame, msg: &str) {
        let area = f.area();
        let width = (msg.len() as u16 + 6).min(area.width - 4).max(20);
        let height = 3;
        let x = area.width.saturating_sub(width + 2);
        let y = 1;
        let toast_area = Rect::new(x, y, width, height);

        f.render_widget(Clear, toast_area);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(THEME.error))
            .style(Style::default().bg(THEME.bg));
        let inner = block.inner(toast_area);
        f.render_widget(block, toast_area);
        let para = Paragraph::new(msg)
            .style(Style::default().fg(THEME.error))
            .alignment(Alignment::Center);
        f.render_widget(para, inner);
    }

    fn draw_help(&self, f: &mut Frame) {
        let area = f.area();
        let popup_area = centered_rect(60, 70, area);
        f.render_widget(Clear, popup_area);

        let block = Block::default()
            .title(Span::styled(
                " Keyboard Shortcuts ",
                Style::default()
                    .fg(THEME.primary)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(THEME.border))
            .style(Style::default().bg(THEME.bg));
        let inner = block.inner(popup_area);
        f.render_widget(block, popup_area);

        let rows = vec![
            Row::new(vec!["↑ / k", "Move up"]),
            Row::new(vec!["↓ / j", "Move down"]),
            Row::new(vec!["Enter", "Select / Confirm"]),
            Row::new(vec!["Tab", "Next field"]),
            Row::new(vec!["Shift+Tab", "Previous field"]),
            Row::new(vec!["Esc", "Go back"]),
            Row::new(vec!["Ctrl+C", "Quit immediately"]),
            Row::new(vec!["?", "Toggle this help"]),
        ];
        let table = Table::new(
            rows,
            [Constraint::Length(12), Constraint::Min(20)],
        )
        .style(Style::default().fg(THEME.fg))
        .column_spacing(2);
        f.render_widget(table, inner);
    }

    // ── Welcome ─────────────────────────────────────────────────────────────

    fn draw_welcome(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(THEME.border));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(30),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(1),
            ])
            .split(inner);

        let banner = r#"
   ___                 ____
  / _ \ _ __   ___ _ __|  _ \ __ ___      __
 | | | | '_ \ / _ \ '_ \ |_) / _` \ \ /\ / /
 | |_| | |_) |  __/ | | |  __/ (_| |\ V  V /
  \___/| .__/ \___|_| |_|_|   \__,_| \_/\_/
       |_|
"#;
        let para = Paragraph::new(banner)
            .style(Style::default().fg(THEME.primary))
            .alignment(Alignment::Center);
        f.render_widget(para, layout[0]);

        let msg = if self.is_edit {
            "Existing configuration found.\nPress Enter to edit or reconfigure."
        } else {
            "Welcome to OpenPaw — your local AI agent.\nPress Enter to start the setup wizard."
        };
        let para2 = Paragraph::new(msg)
            .style(Style::default().fg(THEME.fg))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        f.render_widget(para2, layout[1]);

        let hint = Paragraph::new("[ Press Enter to continue ]")
            .style(Style::default().fg(THEME.muted))
            .alignment(Alignment::Center);
        f.render_widget(hint, layout[2]);
    }

    // ── Provider ────────────────────────────────────────────────────────────

    fn draw_provider(&mut self, f: &mut Frame, area: Rect) {
        match self.sub_stage {
            SubStage::Provider(ProviderStage::Select) => {
                let items: Vec<ListItem> = self
                    .providers
                    .iter()
                    .map(|(_, label, source, model)| {
                        let line = if model.is_empty() {
                            format!("{:<22} [{:^14}]", label, source)
                        } else {
                            format!("{:<22} [{:^14}]  {}", label, source, model)
                        };
                        ListItem::new(line).style(Style::default().fg(THEME.fg))
                    })
                    .collect();
                self.draw_list_screen(f, area, "Select AI Provider", items, "Choose the LLM provider that powers your agent.");
            }
            SubStage::Provider(ProviderStage::GeminiAuth) => {
                let items = vec![
                    ListItem::new("API Key    — Paste your Gemini API key"),
                    ListItem::new("CLI OAuth  — Use existing Gemini CLI login"),
                ];
                self.draw_list_screen(f, area, "Gemini Authentication", items, "How do you want to authenticate with Gemini?");
            }
            SubStage::Provider(ProviderStage::Key) => {
                let title = match self.providers[self.selected_provider_idx].0 {
                    "gemini" => "Gemini API Key",
                    "openai" => "OpenAI API Key",
                    "anthropic" => "Anthropic API Key",
                    "openrouter" => "OpenRouter API Key",
                    "opencode" => "OpenCode API Key",
                    "kilocode" => "Kilocode API Key",
                    _ => "API Key",
                };
                self.draw_input_screen(f, area, title, true, "Paste your API key. It will be masked.");
            }
            SubStage::Provider(ProviderStage::BaseUrl) => {
                self.draw_input_screen(f, area, "Custom Base URL", false, "e.g. http://localhost:8080/v1");
            }
            SubStage::Provider(ProviderStage::Fetching) => {
                self.draw_spinner_screen(f, area, "Fetching available models…");
            }
            SubStage::Provider(ProviderStage::ModelSelect) => {
                let items: Vec<ListItem> = self
                    .model_labels
                    .iter()
                    .map(|label| ListItem::new(label.clone()).style(Style::default().fg(THEME.fg)))
                    .collect();
                self.draw_list_screen(f, area, "Select Model", items, "Choose the model to use for this provider.");
            }
            _ => {}
        }
    }

    // ── Cheap Model ─────────────────────────────────────────────────────────

    fn draw_cheap_model(&mut self, f: &mut Frame, area: Rect) {
        match self.sub_stage {
            SubStage::Cheap(CheapStage::Select) => {
                let items: Vec<ListItem> = self
                    .cheap_providers
                    .iter()
                    .map(|(_, label)| {
                        ListItem::new(*label).style(Style::default().fg(THEME.fg))
                    })
                    .collect();
                self.draw_list_screen(f, area, "Background Tasks Provider", items, "Select a free/cheap provider for background planning to save money.");
            }
            SubStage::Cheap(CheapStage::Key) => {
                let title = match self.cheap_providers[self.selected_cheap_provider_idx].0 {
                    "openrouter" => "OpenRouter API Key",
                    "opencode" => "OpenCode API Key",
                    "kilocode" => "Kilocode API Key",
                    _ => "API Key",
                };
                self.draw_input_screen(f, area, title, true, "API key for the background task provider.");
            }
            SubStage::Cheap(CheapStage::Fetching) => {
                self.draw_spinner_screen(f, area, "Fetching free models…");
            }
            SubStage::Cheap(CheapStage::ModelSelect) => {
                let items: Vec<ListItem> = self
                    .model_labels
                    .iter()
                    .map(|label| ListItem::new(label.clone()).style(Style::default().fg(THEME.fg)))
                    .collect();
                self.draw_list_screen(f, area, "Select Background Model", items, "Choose a model for background tasks.");
            }
            _ => {}
        }
    }

    // ── Timezone ────────────────────────────────────────────────────────────

    fn draw_timezone(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = COMMON_TIMEZONES
            .iter()
            .map(|(tz, label)| {
                let line = format!("{:<24} {}", label, tz);
                ListItem::new(line).style(Style::default().fg(THEME.fg))
            })
            .collect();
        self.draw_list_screen(f, area, "Select Timezone", items, "So the agent knows when it's late and when to reach you.");
    }

    // ── Memory ──────────────────────────────────────────────────────────────

    fn draw_memory(&mut self, f: &mut Frame, area: Rect) {
        match self.sub_stage {
            SubStage::Memory(MemoryStage::SelectBackend) => {
                let items = vec![
                    ListItem::new("SQLite    — Fast local database, supports semantic search"),
                    ListItem::new("Markdown  — Human-readable files in your workspace"),
                    ListItem::new("None      — Ephemeral, no memory between sessions"),
                ];
                self.draw_list_screen(f, area, "Memory Backend", items, "How should your agent remember things?");
            }
            SubStage::Memory(MemoryStage::ConfirmEmbed) => {
                let items = vec![
                    ListItem::new("Yes — Enable vector embeddings for semantic recall"),
                    ListItem::new("No  — Use keyword search only"),
                ];
                self.draw_list_screen(f, area, "Vector Embeddings", items, "Embeddings require a HuggingFace API key.");
            }
            SubStage::Memory(MemoryStage::EmbedKey) => {
                self.draw_input_screen(f, area, "HuggingFace API Key", true, "Get a token at huggingface.co/settings/tokens");
            }
            _ => {}
        }
    }

    // ── Voice ───────────────────────────────────────────────────────────────

    fn draw_voice(&mut self, f: &mut Frame, area: Rect) {
        match self.sub_stage {
            SubStage::Voice(VoiceStage::Confirm) => {
                let items = vec![
                    ListItem::new("Yes — Enable Groq Whisper speech-to-text"),
                    ListItem::new("No  — Skip voice transcription"),
                ];
                self.draw_list_screen(f, area, "Voice Transcription", items, "Optional: transcribe voice messages via Groq Whisper.");
            }
            SubStage::Voice(VoiceStage::Key) => {
                self.draw_input_screen(f, area, "Groq API Key", true, "Get a key at console.groq.com/keys");
            }
            _ => {}
        }
    }

    // ── Channels ────────────────────────────────────────────────────────────

    fn draw_channels(&mut self, f: &mut Frame, area: Rect) {
        match self.sub_stage {
            SubStage::Channel(ChannelStage::Select) => {
                let items = vec![
                    ListItem::new("None      — CLI only, no messaging integration"),
                    ListItem::new("Telegram  — Talk to the agent via a Telegram bot"),
                    ListItem::new("WhatsApp  — Talk via WhatsApp (local bridge)"),
                    ListItem::new("Email     — Talk via Email (SMTP/IMAP)"),
                    ListItem::new("Both      — Telegram + WhatsApp"),
                ];
                self.draw_list_screen(f, area, "Communication Channels", items, "How will you talk to your agent?");
            }
            SubStage::Channel(ChannelStage::Telegram) => {
                let block = Block::default()
                    .title(Span::styled("Telegram Setup", Style::default().fg(THEME.primary).add_modifier(Modifier::BOLD)))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(THEME.border));
                let inner = block.inner(area);
                f.render_widget(block, area);

                let hint = if self.inputs.len() == 3 {
                    "Both: Telegram + WhatsApp"
                } else {
                    "Create your bot at t.me/BotFather"
                };
                let constraints = if self.inputs.len() == 3 {
                    vec![
                        Constraint::Length(2),
                        Constraint::Length(3),
                        Constraint::Length(3),
                        Constraint::Length(3),
                        Constraint::Length(1),
                        Constraint::Min(1),
                    ]
                } else {
                    vec![
                        Constraint::Length(2),
                        Constraint::Length(3),
                        Constraint::Length(3),
                        Constraint::Length(1),
                        Constraint::Min(1),
                    ]
                };
                let layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(constraints)
                    .split(inner);

                let hint_para = Paragraph::new(hint).style(Style::default().fg(THEME.muted));
                f.render_widget(hint_para, layout[0]);
                self.draw_input_widget(f, layout[1], "Bot Token", &self.inputs[0], true, self.focused_input == 0);
                self.draw_input_widget(f, layout[2], "Your Username (e.g. @you)", &self.inputs[1], false, self.focused_input == 1);
                if self.inputs.len() == 3 {
                    self.draw_input_widget(f, layout[3], "WhatsApp Phone (e.g. +1234)", &self.inputs[2], false, self.focused_input == 2);
                }
                let tab_hint = Paragraph::new("[Tab to switch fields, Enter to confirm]")
                    .style(Style::default().fg(THEME.muted));
                f.render_widget(tab_hint, layout[layout.len() - 2]);
            }
            SubStage::Channel(ChannelStage::Whatsapp) => {
                self.draw_input_screen(f, area, "WhatsApp Phone Number", false, "Enter your phone number with country code.");
            }
            SubStage::Channel(ChannelStage::Email) => {
                let block = Block::default()
                    .title(Span::styled("Email Setup", Style::default().fg(THEME.primary).add_modifier(Modifier::BOLD)))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(THEME.border));
                let inner = block.inner(area);
                f.render_widget(block, area);

                let layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), Constraint::Length(3), Constraint::Length(3),
                        Constraint::Length(3), Constraint::Length(3), Constraint::Length(3),
                        Constraint::Length(1),
                        Constraint::Min(1),
                    ])
                    .split(inner);

                let labels = ["Email Address", "App Password", "SMTP Host", "SMTP Port", "IMAP Host", "IMAP Port"];
                for (i, label) in labels.iter().enumerate() {
                    let is_password = *label == "App Password";
                    self.draw_input_widget(f, layout[i], label, &self.inputs[i], is_password, self.focused_input == i);
                }
                let tab_hint = Paragraph::new("[Tab to switch fields, Enter to confirm]")
                    .style(Style::default().fg(THEME.muted));
                f.render_widget(tab_hint, layout[6]);
            }
            _ => {}
        }
    }

    // ── Composio ────────────────────────────────────────────────────────────

    fn draw_composio(&mut self, f: &mut Frame, area: Rect) {
        match self.sub_stage {
            SubStage::Composio(ComposioStage::Confirm) => {
                let items = vec![
                    ListItem::new("Yes — Enable Composio integrations"),
                    ListItem::new("No  — Skip external app connections"),
                ];
                self.draw_list_screen(f, area, "Composio", items, "Connect external apps like GitHub, Gmail, Notion.");
            }
            SubStage::Composio(ComposioStage::Details) => {
                let block = Block::default()
                    .title(Span::styled("Composio Setup", Style::default().fg(THEME.primary).add_modifier(Modifier::BOLD)))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(THEME.border));
                let inner = block.inner(area);
                f.render_widget(block, area);

                let layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Length(3), Constraint::Length(1), Constraint::Min(1)])
                    .split(inner);

                self.draw_input_widget(f, layout[0], "API Key (app.composio.dev)", &self.inputs[0], true, self.focused_input == 0);
                self.draw_input_widget(f, layout[1], "Entity ID", &self.inputs[1], false, self.focused_input == 1);
                let tab_hint = Paragraph::new("[Tab to switch fields, Enter to confirm]")
                    .style(Style::default().fg(THEME.muted));
                f.render_widget(tab_hint, layout[2]);
            }
            _ => {}
        }
    }

    // ── Web Search ──────────────────────────────────────────────────────────

    fn draw_websearch(&mut self, f: &mut Frame, area: Rect) {
        match self.sub_stage {
            SubStage::WebSearch(WebSearchStage::Select) => {
                let items = vec![
                    ListItem::new("Gemini CLI  — Free, uses installed Gemini CLI (recommended)"),
                    ListItem::new("Brave API   — Brave Search API key required"),
                    ListItem::new("DuckDuckGo  — Free but rate-limited"),
                    ListItem::new("None        — Skip web search"),
                ];
                self.draw_list_screen(f, area, "Web Search Provider", items, "How should your agent search the web?");
            }
            SubStage::WebSearch(WebSearchStage::BraveKey) => {
                self.draw_input_screen(f, area, "Brave API Key", true, "Get a key at brave.com/search/api");
            }
            SubStage::WebSearch(WebSearchStage::GeminiCheck) => {
                let block = Block::default()
                    .title(Span::styled("Gemini CLI Not Found", Style::default().fg(THEME.warning).add_modifier(Modifier::BOLD)))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(THEME.warning));
                let inner = block.inner(area);
                f.render_widget(block, area);
                let msg = "Gemini CLI was not found on PATH.\nInstall it with: npm install -g @google/gemini-cli\n\nPress Enter to continue anyway (you can install it later).";
                let para = Paragraph::new(msg)
                    .style(Style::default().fg(THEME.fg))
                    .wrap(Wrap { trim: true });
                f.render_widget(para, inner);
            }
            _ => {}
        }
    }

    // ── Pushover ────────────────────────────────────────────────────────────

    fn draw_pushover(&mut self, f: &mut Frame, area: Rect) {
        match self.sub_stage {
            SubStage::Pushover(PushoverStage::Confirm) => {
                let items = vec![
                    ListItem::new("Yes — Enable Pushover push notifications"),
                    ListItem::new("No  — Skip push notifications"),
                ];
                self.draw_list_screen(f, area, "Pushover", items, "Receive desktop & mobile push alerts.");
            }
            SubStage::Pushover(PushoverStage::Details) => {
                let block = Block::default()
                    .title(Span::styled("Pushover Setup", Style::default().fg(THEME.primary).add_modifier(Modifier::BOLD)))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(THEME.border));
                let inner = block.inner(area);
                f.render_widget(block, area);

                let layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Length(3), Constraint::Length(1), Constraint::Min(1)])
                    .split(inner);

                self.draw_input_widget(f, layout[0], "App Token (pushover.net/apps)", &self.inputs[0], true, self.focused_input == 0);
                self.draw_input_widget(f, layout[1], "User Key (pushover.net)", &self.inputs[1], true, self.focused_input == 1);
                let tab_hint = Paragraph::new("[Tab to switch fields, Enter to confirm]")
                    .style(Style::default().fg(THEME.muted));
                f.render_widget(tab_hint, layout[2]);
            }
            _ => {}
        }
    }

    // ── Summary ─────────────────────────────────────────────────────────────

    fn draw_summary(&mut self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(Span::styled("Configuration Summary", Style::default().fg(THEME.primary).add_modifier(Modifier::BOLD)))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(THEME.border));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let prov = self
            .config
            .provider
            .as_ref()
            .map(|p| {
                let model = self
                    .config
                    .selected_default_model
                    .as_deref()
                    .unwrap_or(&p.default_model);
                format!("{} · {}", capitalize(&p.name), model)
            })
            .unwrap_or_else(|| "not set".to_string());

        let tz = self.config.timezone.as_deref().unwrap_or("UTC");
        let mem = self
            .config
            .memory_backend
            .as_deref()
            .map(|b| {
                if b == "sqlite" && self.config.embed_model.is_some() {
                    "SQLite + embeddings"
                } else {
                    b
                }
            })
            .unwrap_or("sqlite");
        let voice = if self.config.groq_key.is_some() {
            "Groq Whisper"
        } else {
            "disabled"
        };
        let channels = {
            let tg = self
                .config
                .telegram
                .as_ref()
                .and_then(|t| t.as_ref())
                .map(|(_, u)| u.as_str());
            let wa = self
                .config
                .whatsapp_native
                .as_ref()
                .and_then(|w| w.as_ref())
                .map(|(_, p)| p.as_str());
            match (tg, wa) {
                (Some(t), Some(w)) => format!("Telegram ({}) + WhatsApp ({})", t, w),
                (Some(t), None) => format!("Telegram ({})", t),
                (None, Some(w)) => format!("WhatsApp ({})", w),
                _ => "CLI only".to_string(),
            }
        };
        let search = if self.config.search_provider.as_deref() == Some("brave") {
            "Brave"
        } else {
            self.config.search_provider.as_deref().unwrap_or("gemini_cli")
        };
        let push = if self.config.pushover.as_ref().and_then(|p| p.as_ref()).is_some() {
            "enabled"
        } else {
            "disabled"
        };

        let rows = vec![
            Row::new(vec!["Provider", &prov]),
            Row::new(vec!["Timezone", tz]),
            Row::new(vec!["Memory", mem]),
            Row::new(vec!["Voice", voice]),
            Row::new(vec!["Channels", &channels]),
            Row::new(vec!["Web Search", search]),
            Row::new(vec!["Pushover", push]),
        ];

        let table = Table::new(rows, [Constraint::Length(14), Constraint::Min(20)])
            .header(
                Row::new(vec!["Setting", "Value"])
                    .style(Style::default().add_modifier(Modifier::BOLD).fg(THEME.primary))
                    .bottom_margin(1),
            )
            .style(Style::default().fg(THEME.fg))
            .column_spacing(2);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(12)])
            .split(inner);

        f.render_widget(table, chunks[0]);

        let action_items: Vec<ListItem> = if self.is_edit {
            vec![
                ListItem::new("Edit AI Provider & Model"),
                ListItem::new("Edit Background Tasks"),
                ListItem::new("Edit Timezone"),
                ListItem::new("Edit Memory & Embeddings"),
                ListItem::new("Edit Voice"),
                ListItem::new("Edit Channels"),
                ListItem::new("Edit Composio"),
                ListItem::new("Edit Web Search"),
                ListItem::new("Edit Pushover"),
                ListItem::new("Save & Exit").style(Style::default().fg(THEME.success).add_modifier(Modifier::BOLD)),
                ListItem::new("Discard & Exit").style(Style::default().fg(THEME.error)),
            ]
        } else {
            vec![
                ListItem::new("Save & Exit").style(Style::default().fg(THEME.success).add_modifier(Modifier::BOLD)),
                ListItem::new("Go Back"),
                ListItem::new("Discard").style(Style::default().fg(THEME.error)),
            ]
        };

        let actions = List::new(action_items)
            .block(
                Block::default()
                    .title("Actions")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(THEME.border)),
            )
            .highlight_style(
                Style::default()
                    .bg(THEME.highlight_bg)
                    .fg(THEME.highlight_fg)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("  ▸ ");
        f.render_stateful_widget(actions, chunks[1], &mut self.summary_list);
    }

    // ── Done ────────────────────────────────────────────────────────────────

    fn draw_done(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(THEME.success));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(40),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(1),
            ])
            .split(inner);

        let check = Paragraph::new("✓ Setup Complete!")
            .style(Style::default().fg(THEME.success).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center);
        f.render_widget(check, layout[1]);

        let msg = Paragraph::new("Press Enter to save your configuration and exit.\nRun `openpaw agent` to start.")
            .style(Style::default().fg(THEME.fg))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        f.render_widget(msg, layout[2]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Reusable drawing helpers
    // ═════════════════════════════════════════════════════════════════════════

    fn draw_list_screen(
        &mut self,
        f: &mut Frame,
        area: Rect,
        title: &str,
        items: Vec<ListItem>,
        hint: &str,
    ) {
        let block = Block::default()
            .title(Span::styled(title, Style::default().fg(THEME.primary).add_modifier(Modifier::BOLD)))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(THEME.border));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(1)])
            .split(inner);

        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .bg(THEME.highlight_bg)
                    .fg(THEME.highlight_fg)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("  ▸ ")
            .highlight_spacing(HighlightSpacing::Always);
        f.render_stateful_widget(list, chunks[0], &mut self.list_state);

        let hint_para = Paragraph::new(hint)
            .style(Style::default().fg(THEME.muted))
            .wrap(Wrap { trim: true });
        f.render_widget(hint_para, chunks[1]);
    }

    fn draw_input_screen(
        &mut self,
        f: &mut Frame,
        area: Rect,
        title: &str,
        is_password: bool,
        hint: &str,
    ) {
        let block = Block::default()
            .title(Span::styled(title, Style::default().fg(THEME.primary).add_modifier(Modifier::BOLD)))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(THEME.border));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(3), Constraint::Length(2)])
            .margin(1)
            .split(inner);

        let hint_para = Paragraph::new(hint).style(Style::default().fg(THEME.muted));
        f.render_widget(hint_para, chunks[0]);

        if !self.inputs.is_empty() {
            self.draw_input_widget(f, chunks[1], title, &self.inputs[0], is_password, true);
        }

        let enter_hint = Paragraph::new("[ Press Enter to confirm ]")
            .style(Style::default().fg(THEME.muted));
        f.render_widget(enter_hint, chunks[2]);
    }

    fn draw_spinner_screen(&self, f: &mut Frame, area: Rect, message: &str) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(THEME.border));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let frame = frames[self.tick as usize % frames.len()];
        let text = format!("{}  {}", frame, message);
        let para = Paragraph::new(text)
            .style(Style::default().fg(THEME.primary))
            .alignment(Alignment::Center);
        f.render_widget(para, inner);
    }

    fn draw_input_widget(
        &self,
        f: &mut Frame,
        area: Rect,
        title: &str,
        input: &Input,
        is_password: bool,
        is_focused: bool,
    ) {
        let display_value = if is_password {
            input.value().chars().map(|_| '•').collect::<String>()
        } else {
            input.value().to_string()
        };

        let border_style = if is_focused {
            Style::default().fg(THEME.primary).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(THEME.border)
        };

        let block = Block::default()
            .title(Span::styled(
                title,
                Style::default().fg(if is_focused { THEME.primary } else { THEME.fg }),
            ))
            .borders(Borders::ALL)
            .border_style(border_style);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let scroll = input.visual_scroll(inner.width as usize);
        let para = Paragraph::new(display_value)
            .scroll((0, scroll as u16))
            .style(Style::default().fg(THEME.fg));
        f.render_widget(para, inner);

        if is_focused {
            let cursor_pos = input.visual_cursor().saturating_sub(scroll);
            f.set_cursor_position(Position::new(
                inner.x + cursor_pos as u16,
                inner.y,
            ));
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}
