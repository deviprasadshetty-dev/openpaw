pub enum IntegrationStatus {
    Available,
    Active,
    ComingSoon,
}

pub enum IntegrationCategory {
    Chat,
    AiModel,
    Productivity,
    MusicAudio,
    SmartHome,
    ToolsAutomation,
    MediaCreative,
    Social,
    Platform,
}

pub struct IntegrationEntry {
    pub name: &'static str,
    pub description: &'static str,
    pub category: IntegrationCategory,
    pub status: IntegrationStatus,
}

pub const ALL_INTEGRATIONS: &[IntegrationEntry] = &[
    IntegrationEntry {
        name: "Telegram",
        description: "Bot API - long-polling",
        category: IntegrationCategory::Chat,
        status: IntegrationStatus::Available,
    },
    IntegrationEntry {
        name: "OpenAI",
        description: "GPT-4o, GPT-5, o1",
        category: IntegrationCategory::AiModel,
        status: IntegrationStatus::Available,
    },
    IntegrationEntry {
        name: "Anthropic",
        description: "Claude 3.5/4 Sonnet & Opus",
        category: IntegrationCategory::AiModel,
        status: IntegrationStatus::Available,
    },
    IntegrationEntry {
        name: "Ollama",
        description: "Local models (Llama, etc.)",
        category: IntegrationCategory::AiModel,
        status: IntegrationStatus::Available,
    },
];

pub fn all_integrations() -> &'static [IntegrationEntry] {
    ALL_INTEGRATIONS
}
