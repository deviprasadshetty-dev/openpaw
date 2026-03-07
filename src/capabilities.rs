use crate::build_options;
use crate::channel_catalog;
use crate::config::Config;
use crate::memory::engines::registry as memory_registry;
use crate::tools::root::Tool;
use serde_json::json;

const CORE_TOOL_NAMES: &[&str] = &[
    "shell",
    "file_read",
    "file_write",
    "file_edit",
    "git",
    "image_info",
    "memory_store",
    "memory_recall",
    "memory_list",
    "memory_forget",
    "delegate",
    "schedule",
    "spawn",
];

const OPTIONAL_TOOL_NAMES: &[&str] = &[
    "http_request",
    "browser",
    "screenshot",
    "composio",
    "browser_open",
    "hardware_board_info",
    "hardware_memory",
    "i2c",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMode {
    BuildEnabled,
    BuildDisabled,
    Configured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineMode {
    BuildEnabled,
    BuildDisabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionalToolMode {
    Enabled,
    Disabled,
}

fn runtime_has_tool(runtime_tools: Option<&[Box<dyn Tool>]>, name: &str) -> bool {
    if let Some(tools) = runtime_tools {
        for t in tools {
            if t.name() == name {
                return true;
            }
        }
    }
    false
}

fn optional_tool_enabled_by_config(cfg: &Config, name: &str) -> bool {
    match name {
        "http_request" => cfg.http_request.enabled,
        "browser" => cfg.browser.enabled,
        "screenshot" => cfg.browser.enabled,
        "composio" => cfg.composio.enabled && cfg.composio.api_key.is_some(),
        "browser_open" => !cfg.browser.allowed_domains.is_empty(),
        "hardware_board_info" => cfg.hardware.enabled,
        "hardware_memory" => cfg.hardware.enabled,
        "i2c" => cfg.hardware.enabled,
        _ => false,
    }
}

fn collect_channel_names(
    cfg_opt: Option<&Config>,
    mode: ChannelMode,
) -> Vec<String> {
    let mut out = Vec::new();

    for meta in channel_catalog::KNOWN_CHANNELS {
        let enabled = channel_catalog::is_build_enabled(meta.id);
        let configured = if let Some(cfg) = cfg_opt {
            channel_catalog::configured_count(cfg, meta.id) > 0
        } else {
            false
        };

        let include = match mode {
            ChannelMode::BuildEnabled => enabled,
            ChannelMode::BuildDisabled => !enabled,
            ChannelMode::Configured => enabled && configured,
        };

        if include {
            out.push(meta.key.to_string());
        }
    }
    out
}

fn collect_memory_engine_names(mode: EngineMode) -> Vec<String> {
    let mut out = Vec::new();

    for name in memory_registry::KNOWN_BACKEND_NAMES {
        // Assuming all known backends are build-enabled if they are in the list,
        // but Zig checked findBackend which returned null if not enabled via build options.
        // My registry.rs includes all for now, but I should probably check if they are "enabled".
        // In my registry implementation, find_backend returns the descriptor if found.
        // Assuming all in KNOWN_BACKEND_NAMES are available.
        let enabled = memory_registry::find_backend(name).is_some();
        
        let include = match mode {
            EngineMode::BuildEnabled => enabled,
            EngineMode::BuildDisabled => !enabled,
        };

        if include {
            out.push(name.to_string());
        }
    }
    out
}

fn collect_optional_tools(
    cfg_opt: Option<&Config>,
    mode: OptionalToolMode,
) -> Vec<String> {
    let mut out = Vec::new();

    let cfg = if let Some(c) = cfg_opt {
        c
    } else {
        if mode == OptionalToolMode::Disabled {
            for name in OPTIONAL_TOOL_NAMES {
                out.push(name.to_string());
            }
        }
        return out;
    };

    for name in OPTIONAL_TOOL_NAMES {
        let enabled = optional_tool_enabled_by_config(cfg, name);
        let include = match mode {
            OptionalToolMode::Enabled => enabled,
            OptionalToolMode::Disabled => !enabled,
        };
        if include {
            out.push(name.to_string());
        }
    }
    out
}

fn collect_runtime_tool_names(
    cfg_opt: Option<&Config>,
    runtime_tools: Option<&[Box<dyn Tool>]>,
) -> Vec<String> {
    if let Some(tools) = runtime_tools {
        return tools.iter().map(|t| t.name().to_string()).collect();
    }

    let mut estimated = Vec::new();
    for name in CORE_TOOL_NAMES {
        estimated.push(name.to_string());
    }
    let optional_enabled = collect_optional_tools(cfg_opt, OptionalToolMode::Enabled);
    estimated.extend(optional_enabled);
    estimated
}

pub fn build_manifest_json(
    cfg_opt: Option<&Config>,
    runtime_tools: Option<&[Box<dyn Tool>]>,
) -> String {
    let runtime_loaded_names = if runtime_tools.is_some() {
        collect_runtime_tool_names(cfg_opt, runtime_tools)
    } else {
        Vec::new()
    };

    let estimated_tool_names = if runtime_tools.is_none() {
        collect_runtime_tool_names(cfg_opt, None)
    } else {
        Vec::new()
    };

    let channels_data: Vec<serde_json::Value> = channel_catalog::KNOWN_CHANNELS.iter().map(|meta| {
        let enabled = channel_catalog::is_build_enabled(meta.id);
        let configured_count = if let Some(cfg) = cfg_opt {
            channel_catalog::configured_count(cfg, meta.id)
        } else {
            0
        };
        let configured = enabled && configured_count > 0;
        
        json!({
            "key": meta.key,
            "label": meta.label,
            "enabled_in_build": enabled,
            "configured": configured,
            "configured_count": configured_count
        })
    }).collect();

    let memory_engines_data: Vec<serde_json::Value> = memory_registry::KNOWN_BACKEND_NAMES.iter().map(|name| {
        let enabled = memory_registry::find_backend(name).is_some();
        let configured = if let Some(cfg) = cfg_opt {
            cfg.memory.backend == *name
        } else {
            false
        };
        
        json!({
            "name": name,
            "enabled_in_build": enabled,
            "configured": configured
        })
    }).collect();

    let optional_enabled = collect_optional_tools(cfg_opt, OptionalToolMode::Enabled);
    let optional_disabled = collect_optional_tools(cfg_opt, OptionalToolMode::Disabled);

    let active_memory_backend = if let Some(cfg) = cfg_opt {
        cfg.memory.backend.clone()
    } else {
        String::new()
    };

    json!({
        "version": build_options::VERSION,
        "active_memory_backend": active_memory_backend,
        "channels": channels_data,
        "memory_engines": memory_engines_data,
        "tools": {
            "runtime_loaded": runtime_loaded_names,
            "estimated_enabled_from_config": estimated_tool_names,
            "optional_enabled_by_config": optional_enabled,
            "optional_disabled_by_config": optional_disabled
        }
    }).to_string()
}

pub fn build_summary_text(
    cfg_opt: Option<&Config>,
    runtime_tools: Option<&[Box<dyn Tool>]>,
) -> String {
    let channels_enabled = collect_channel_names(cfg_opt, ChannelMode::BuildEnabled).join(", ");
    let channels_disabled = collect_channel_names(cfg_opt, ChannelMode::BuildDisabled).join(", ");
    let channels_configured = collect_channel_names(cfg_opt, ChannelMode::Configured).join(", ");

    let engines_enabled = collect_memory_engine_names(EngineMode::BuildEnabled).join(", ");
    let engines_disabled = collect_memory_engine_names(EngineMode::BuildDisabled).join(", ");

    let runtime_tool_names = collect_runtime_tool_names(cfg_opt, runtime_tools).join(", ");
    let optional_disabled = collect_optional_tools(cfg_opt, OptionalToolMode::Disabled).join(", ");

    let active_backend = if let Some(cfg) = cfg_opt {
        cfg.memory.backend.clone()
    } else {
        "(unknown)".to_string()
    };

    let tools_label = if runtime_tools.is_some() {
        "tools (loaded)"
    } else {
        "tools (estimated from config)"
    };

    format!(
        "Capabilities\n\n\
        Available in this runtime:\n  \
        channels (build): {}\n  \
        channels (configured): {}\n  \
        memory engines (build): {}\n  \
        active memory backend: {}\n  \
        {}: {}\n\n\
        Not available in this runtime:\n  \
        channels (disabled in build): {}\n  \
        memory engines (disabled in build): {}\n  \
        optional tools (disabled by config): {}\n",
        channels_enabled,
        channels_configured,
        engines_enabled,
        active_backend,
        tools_label,
        runtime_tool_names,
        channels_disabled,
        engines_disabled,
        optional_disabled
    )
}
