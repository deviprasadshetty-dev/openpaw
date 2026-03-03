use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use crate::agent::Tool;

pub const BOOTSTRAP_MAX_CHARS: usize = 20_000;
pub const MAX_WORKSPACE_BOOTSTRAP_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ConversationContext {
    pub channel: Option<String>,
    pub sender_number: Option<String>,
    pub sender_uuid: Option<String>,
    pub group_id: Option<String>,
    pub is_group: Option<bool>,
}

pub struct PromptContext<'a> {
    pub workspace_dir: &'a str,
    pub model_name: &'a str,
    pub tools: &'a [Arc<dyn Tool>],
    pub capabilities_section: Option<&'a str>,
    pub conversation_context: Option<&'a ConversationContext>,
    pub use_native_tools: bool,
}

fn path_starts_with(path: &Path, prefix: &Path) -> bool {
    path.starts_with(prefix)
}

fn is_workspace_bootstrap_filename_safe(filename: &str) -> bool {
    if Path::new(filename).is_absolute() {
        return false;
    }
    if filename.contains('\0') {
        return false;
    }
    for part in filename.split(|c| c == '/' || c == '\\') {
        if part == ".." {
            return false;
        }
    }
    true
}

fn open_workspace_file_guarded(workspace_dir: &str, filename: &str) -> Option<(fs::File, PathBuf)> {
    if !is_workspace_bootstrap_filename_safe(filename) {
        return None;
    }

    let workspace_root = fs::canonicalize(workspace_dir).ok()?;
    let candidate = Path::new(workspace_dir).join(filename);
    let canonical_path = fs::canonicalize(candidate).ok()?;

    if !path_starts_with(&canonical_path, &workspace_root) {
        return None;
    }

    let file = fs::File::open(&canonical_path).ok()?;
    let metadata = file.metadata().ok()?;
    
    if metadata.len() > MAX_WORKSPACE_BOOTSTRAP_FILE_BYTES {
        return None;
    }

    Some((file, canonical_path))
}

pub fn workspace_prompt_fingerprint(workspace_dir: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    let tracked_files = [
        "AGENTS.md",
        "SOUL.md",
        "TOOLS.md",
        "IDENTITY.md",
        "USER.md",
        "HEARTBEAT.md",
        "BOOTSTRAP.md",
        "MEMORY.md",
        "memory.md",
    ];

    for filename in tracked_files {
        hasher.write(filename.as_bytes());
        hasher.write(b"\n");

        if let Some((file, path)) = open_workspace_file_guarded(workspace_dir, filename) {
            hasher.write(b"present");
            hasher.write(path.to_string_lossy().as_bytes());

            if let Ok(metadata) = file.metadata() {
                hasher.write_u64(metadata.len());
                if let Ok(modified) = metadata.modified() {
                    // Best effort hashing of mtime
                    if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
                        hasher.write_u64(duration.as_secs());
                        hasher.write_u32(duration.subsec_nanos());
                    }
                }
            }
        } else {
            hasher.write(b"missing");
        }
    }

    hasher.finish()
}

pub fn build_system_prompt(ctx: PromptContext) -> String {
    let mut out = String::new();

    // Identity section
    build_identity_section(&mut out, ctx.workspace_dir);

    // Tools section
    build_tools_section(&mut out, ctx.tools, ctx.use_native_tools);

    // Attachments section
    append_channel_attachments_section(&mut out);

    // Conversation context
    if let Some(cc) = ctx.conversation_context {
        out.push_str("## Conversation Context\n\n");
        if let Some(ch) = &cc.channel {
            out.push_str(&format!("- Channel: {}\n", ch));
        }
        if let Some(is_group) = cc.is_group {
            if is_group {
                if let Some(gid) = &cc.group_id {
                    out.push_str("- Chat type: group\n");
                    out.push_str(&format!("- Group ID: {}\n", gid));
                } else {
                    out.push_str("- Chat type: group\n");
                }
            } else {
                out.push_str("- Chat type: direct message\n");
            }
        }
        if let Some(num) = &cc.sender_number {
            out.push_str(&format!("- Sender phone: {}\n", num));
        }
        if let Some(uuid) = &cc.sender_uuid {
            out.push_str(&format!("- Sender UUID: {}\n", uuid));
        }
        out.push_str("\n");
    }

    if let Some(caps) = ctx.capabilities_section {
        out.push_str(caps);
    }

    // Safety section
    out.push_str("## Safety\n\n");
    out.push_str("- Do not exfiltrate private data.\n");
    out.push_str("- Do not run destructive commands without asking.\n");
    out.push_str("- Do not bypass oversight or approval mechanisms.\n");
    out.push_str("- Prefer `trash` over `rm`.\n");
    out.push_str("- When in doubt, ask before acting externally.\n\n");
    out.push_str("- Never expose internal memory implementation keys (for example: `autosave_*`, `last_hygiene_at`) in user-facing replies.\n\n");

    // Skills section (Placeholder - would need skills module)
    // append_skills_section(&mut out, ctx.workspace_dir); 
    
    // Workspace section
    out.push_str(&format!("## Workspace\n\nWorking directory: `{}`\n\n", ctx.workspace_dir));

    // Runtime section
    out.push_str(&format!("## Runtime\n\nOS: {} | Model: {}\n\n", std::env::consts::OS, ctx.model_name));

    out
}

fn inject_workspace_file(out: &mut String, workspace_dir: &str, filename: &str) {
    if let Some((_, path)) = open_workspace_file_guarded(workspace_dir, filename) {
        if let Ok(content) = fs::read_to_string(path) {
            if content.trim().is_empty() { return; }
            
            out.push_str(&format!("\n=== BEGIN {} ===\n", filename));
            if content.len() > BOOTSTRAP_MAX_CHARS {
                out.push_str(&content[..BOOTSTRAP_MAX_CHARS]);
                out.push_str("\n...[truncated]...\n");
            } else {
                out.push_str(&content);
            }
            out.push_str(&format!("\n=== END {} ===\n\n", filename));
        }
    }
}

fn build_identity_section(out: &mut String, workspace_dir: &str) {
    out.push_str("## Project Context\n\n");
    out.push_str("The following workspace files define your identity, behavior, and context.\n\n");
    out.push_str("If AGENTS.md is present, follow its operational guidance (including startup routines and red-line constraints) unless higher-priority instructions override it.\n\n");
    out.push_str("If SOUL.md is present, embody its persona and tone. Avoid stiff, generic replies; follow its guidance unless higher-priority instructions override it.\n\n");
    out.push_str("TOOLS.md does not control tool availability; it is user guidance for how to use external tools.\n\n");

    let identity_files = [
        "AGENTS.md",
        "SOUL.md",
        "TOOLS.md",
        "IDENTITY.md",
        "USER.md",
        "HEARTBEAT.md",
        "BOOTSTRAP.md",
    ];

    for filename in identity_files {
        inject_workspace_file(out, workspace_dir, filename);
    }
    
    // Memory file preference
    if open_workspace_file_guarded(workspace_dir, "MEMORY.md").is_some() {
        inject_workspace_file(out, workspace_dir, "MEMORY.md");
    } else {
        inject_workspace_file(out, workspace_dir, "memory.md");
    }
}

fn build_tools_section(out: &mut String, tools: &[Arc<dyn Tool>], use_native_tools: bool) {
    out.push_str("## Tools\n\n");
    for tool in tools {
        out.push_str(&format!("- **{}**: {}\n  Parameters: `{}`\n",
            tool.name(),
            tool.description(),
            tool.parameters_json()
        ));
    }

    // Only add tool calling instructions for non-native tool providers
    if !use_native_tools && !tools.is_empty() {
        out.push_str("\n## Tool Calling Instructions\n\n");
        out.push_str("To call a tool, output a tool call block in the following XML format:\n\n");
        out.push_str("<tool_call>{\"name\": \"tool_name\", \"arguments\": {\"param1\": \"value1\", \"param2\": \"value2\"}}</tool_call>\n\n");
        out.push_str("Important:\n");
        out.push_str("- The tool call must be valid JSON inside the XML tags\n");
        out.push_str("- The 'name' field must match one of the available tools listed above\n");
        out.push_str("- The 'arguments' object must match the tool's parameter schema\n");
        out.push_str("- You can include multiple tool calls in your response\n");
        out.push_str("- After tool calls are executed, you will receive the results and can respond to the user\n\n");
        out.push_str("When NOT to use tools:\n");
        out.push_str("- For simple greetings, casual conversation, or general questions that don't require external data\n");
        out.push_str("- When the user is just saying hello, asking how you are, or making small talk\n");
        out.push_str("- When you already have all the information needed to answer from the conversation context\n");
        out.push_str("- Only use tools when you need to perform actions like reading files, executing commands, or accessing external data\n\n");
    }

    out.push_str("\n");
}

fn append_channel_attachments_section(out: &mut String) {
    out.push_str("## Channel Attachments\n\n");
    out.push_str("- On marker-aware channels (for example Telegram), you can send real attachments by emitting markers in your final reply.\n");
    out.push_str("- File/document: `[FILE:/absolute/path/to/file.ext]` or `[DOCUMENT:/absolute/path/to/file.ext]`\n");
    out.push_str("- Image/video/audio/voice: `[IMAGE:/abs/path]`, `[VIDEO:/abs/path]`, `[AUDIO:/abs/path]`, `[VOICE:/abs/path]`\n");
    out.push_str("- If user gives `~/...`, expand it to the absolute home path before sending.\n");
    out.push_str("- Do not claim attachment sending is unavailable when these markers are supported.\n\n");

    out.push_str("## Channel Choices\n\n");
    out.push_str("- On supported channels (for example Telegram when enabled), append `<nc_choices>...</nc_choices>` at the end of the final reply to render short button choices when you are asking the user to choose among short options.\n");
    out.push_str("- Always keep the normal visible question text before the choices block.\n");
    out.push_str("- Use choices only for short mutually exclusive branches (for example yes/no or A/B).\n");
    out.push_str("- Do not use choices for long lists, open-ended prompts, or complex multi-step forms.\n");
    out.push_str("- If you ask the user to pick one of 2-4 short explicit options (for example yes/no/cancel, A/B, or quoted command replies), you MUST append a choices block unless the user explicitly asked for plain text only.\n");
    out.push_str("- If you present a numbered or bulleted list of 2-4 mutually exclusive reply options, include matching choices for those same options.\n");
    out.push_str("- The JSON must be valid and use `{\"v\":1,\"options\":[...]}` with 2-6 options.\n");
    out.push_str("- Each option must include `id` and `label`; `submit_text` is optional (if omitted, label is used as submit text).\n");
    out.push_str("- `id` must be lowercase and contain only `a-z`, `0-9`, `_`, `-` (example: `yes`, `no`, `later_10m`).\n");
    out.push_str("- Example: `<nc_choices>{\"v\":1,\"options\":[{\"id\":\"yes\",\"label\":\"Yes\",\"submit_text\":\"Yes\"},{\"id\":\"no\",\"label\":\"No\"}]}</nc_choices>`\n\n");
}
