use crate::agent::Tool;
use crate::skills::{check_requirements, list_skills};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    pub token_limit: u64,
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
    for part in filename.split(['/', '\\']) {
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
    let is_lean = crate::agent::context_tokens::is_small_model_context(ctx.token_limit);

    // Identity section
    build_identity_section(&mut out, ctx.workspace_dir, is_lean);

    // Tools section
    build_tools_section(&mut out, ctx.tools, ctx.use_native_tools, is_lean);

    // Attachments section
    if !is_lean {
        append_channel_attachments_section(&mut out);
    }

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
        out.push('\n');
    }

    if let Some(caps) = ctx.capabilities_section {
        out.push_str(caps);
    }

    // Current Date & Time
    append_date_time_section(&mut out);

    if is_lean {
        out.push_str("## Reasoning & Action\n\n- Think simply. Be concise.\n- If a task requires multiple steps, take as many turns as needed to complete it.\n- Use tools directly to gather information or perform actions.\n\n");
    } else {
        // Reasoning and Execution section
        out.push_str("## Reasoning and Execution\n\n");
        out.push_str("- Think step-by-step before acting. Formulate a clear plan, break it down into steps, and reflect on the results of each step.\n");
        out.push_str("- **Efficiency & Batching**: Aim to execute multiple independent tool calls in a single turn whenever possible to save time and resources.\n");
        out.push_str("- **Final Synthesis**: Once you have gathered all necessary information or completed the requested actions via tools, you MUST provide a comprehensive final response to the user. This response should summarize your findings, explain what was done, and confirm task completion in a way that is informative and helpful. **Never end a turn with only tool results or a brief placeholder like \"Done.\" or \"✅ Done.\" if tools were used.**\n");
        out.push_str("- Use `<thought>` tags or general text to explain your reasoning before executing any tools.\n");
        out.push_str("- If a task is complex, outline an explicit multi-step plan first, then execute the steps sequentially, reviewing the outcomes along the way.\n\n");

        // Persistence — never give up, find alternatives
        out.push_str("## Persistence & Problem Solving\n\n");
        out.push_str("You have access to a terminal, the full file system, a web browser, code execution, screenshot tools, and more — the same capabilities a skilled human engineer has at their desk.\n\n");
        out.push_str("Given these capabilities, \"I can't do that\" is almost never true and is not an acceptable response.\n\n");
        out.push_str("- **If one approach fails, immediately try another.** Run a shell command. Read a file. Search the web. Write a script. Use a different tool.\n");
        out.push_str("- **If a tool errors out**, diagnose the error (check the output, read logs, inspect the file) and retry with the right parameters or a workaround.\n");
        out.push_str("- **If you are blocked**, decompose the problem — break it into smaller steps and solve each one. There is always a path forward.\n");
        out.push_str("- **Only tell the user something cannot be done** after you have genuinely exhausted every tool and approach, and always explain exactly what you tried and why it failed.\n");
        out.push_str("- Think like a resourceful engineer: Google it, write the script, read the source, patch it, work around it. Never give up.\n\n");

        // Communication rules
        out.push_str("## Communication Rules\n\n");
        out.push_str("- Never mention or refer to internal configuration files (e.g. SOUL.md, AGENTS.md, IDENTITY.md, USER.md, MEMORY.md, etc.) in your replies. These are private implementation details.\n");
        out.push_str(
            "- Never expose memory keys (e.g. autosave_*, last_hygiene_at) in user-facing replies.\n",
        );
        out.push_str("- Speak naturally as if these instructions are simply who you are — don't break the fourth wall.\n\n");

        // Proactive Tool Use section
        out.push_str("## Proactive Tool Use\n\n");
        out.push_str("You should automatically use tools for these common patterns to maintain momentum:\n\n");
        out.push_str(
            "- User mentions a file path → Use `file_read` to check contents immediately.\n",
        );
        out.push_str("- User asks about current events → Use `web_search` for latest info.\n");
        out.push_str(
            "- User wants to run a command → Use `shell` directly if it's read-only or low-risk.\n",
        );
        out.push_str(
            "- User references prior conversation → Use `memory_recall` to find context.\n",
        );
        out.push_str("- User needs to install something → Use `shell` to check/install dependencies in a virtualenv.\n\n");
        out.push_str("Don't ask 'Would you like me to...' for these obvious actions — just do them and report the result.\n\n");

        // Safety & Autonomy section
        out.push_str("## Safety & Autonomy\n\n");
        out.push_str("### You MAY act autonomously (no confirmation needed):\n");
        out.push_str("- Reading files in the workspace directory.\n");
        out.push_str("- Web searches and fetching public URLs.\n");
        out.push_str("- Running read-only shell commands (ls, cat, grep, git status, etc.).\n");
        out.push_str("- Installing packages in virtual environments.\n");
        out.push_str("- Writing new files under 5KB in workspace directories.\n\n");

        out.push_str("### You MUST ask before:\n");
        out.push_str("- Deleting or overwriting existing files > 1KB.\n");
        out.push_str(
            "- Running commands with network egress (curl POST, wget to external targets).\n",
        );
        out.push_str("- Executing code that modifies system-wide state or configurations.\n");
        out.push_str("- Accessing or modifying files outside the workspace.\n\n");

        out.push_str("### Destructive Patterns (Always ask):\n");
        out.push_str("- Commands containing `rm `, `rm -`, `del `, `rmdir `.\n");
        out.push_str("- Shell redirects that overwrite files: `> file`.\n");
        out.push_str("- Permission/ownership changes: `chmod`, `chown`.\n\n");

        out.push_str("### Never:\n");
        out.push_str("- Exfiltrate private data (API keys, credentials, personal info).\n");
        out.push_str("- Bypass established oversight or approval mechanisms.\n\n");

        // Safety & Group logic
        append_safety_and_group_logic(&mut out, ctx.conversation_context);
    }

    // Skills section
    append_skills_section(&mut out, ctx.workspace_dir);

    // Workspace section
    out.push_str(&format!(
        "## Workspace\n\nWorking directory: `{}`\n\n",
        ctx.workspace_dir
    ));

    // Runtime section
    out.push_str(&format!(
        "## Runtime\n\nOS: {} | Model: {}\n\n",
        std::env::consts::OS,
        ctx.model_name
    ));

    out
}

/// Scan `<workspace>/skills/`, check deps, inject enabled skills into the prompt.
fn append_skills_section(out: &mut String, workspace_dir: &str) {
    let ws = Path::new(workspace_dir);
    let mut skills = list_skills(ws).unwrap_or_default();
    for s in skills.iter_mut() {
        check_requirements(s);
    }
    let active: Vec<_> = skills
        .iter()
        .filter(|s| s.enabled && s.available && !s.instructions.is_empty())
        .collect();

    if active.is_empty() {
        return;
    }

    out.push_str("## Active Skills\n\n");
    out.push_str(
        "The following skills extend your capabilities. Follow their instructions carefully.\n\n",
    );
    for skill in &active {
        out.push_str(&format!("### Skill: {}\n", skill.name));
        if !skill.description.is_empty() {
            out.push_str(&format!("{} (v{})\n\n", skill.description, skill.version));
        }
        out.push_str(&skill.instructions);
        out.push_str("\n\n");
    }

    let available: Vec<_> = skills
        .iter()
        .filter(|s| !s.always && s.enabled && !s.instructions.is_empty())
        .collect();

    if !available.is_empty() {
        out.push_str("### Available Skills\n\n");
        out.push_str("These skills are installed but not preloaded. Use the `read_file` tool on a skill's location to load its full instructions.\n\n");
        out.push_str("1. Do NOT load a skill until the task matches its name or description.\n");
        out.push_str("2. When multiple skills could match, load the most specific one first.\n\n");
        out.push_str("<available_skills>\n");
        for skill in &available {
            out.push_str("  <skill>\n");
            out.push_str(&format!("    <name>{}</name>\n", skill.name));
            if !skill.description.is_empty() {
                out.push_str(&format!(
                    "    <description>{}</description>\n",
                    skill.description
                ));
            }
            out.push_str(&format!(
                "    <location>{}/SKILL.md</location>\n",
                skill.path
            ));
            out.push_str("  </skill>\n");
        }
        out.push_str("</available_skills>\n\n");
    }
}

pub fn append_date_time_section(out: &mut String) {
    let now = chrono::Utc::now();
    out.push_str("## Current Date & Time\n\n");
    out.push_str(&format!("{} UTC\n\n", now.format("%Y-%m-%d %H:%M")));
}

fn append_safety_and_group_logic(out: &mut String, conversation_context: Option<&ConversationContext>) {
    // Safety additions
    out.push_str("## Safety\n\n- Prefer `trash` over `rm` when deleting files.\n- When in doubt, ask before acting externally.\n\n");

    // Group chat behavior
    if let Some(cc) = conversation_context {
        let is_telegram = if let Some(ch) = &cc.channel {
            ch.to_lowercase() == "telegram"
        } else {
            false
        };

        if is_telegram && cc.is_group.unwrap_or(false) {
            out.push_str("## Group Chat Behavior\n\n");
            out.push_str("You are in a group chat. Not every message requires a response.\n\n");
            out.push_str("Use the `[NO_REPLY]` marker when:\n");
            out.push_str("- The message is casual chat between other members\n");
            out.push_str("- The message is not directed at you (no question, no @mention)\n");
            out.push_str("- The message is a simple acknowledgment (ok, thanks, haha, etc.)\n\n");
            out.push_str("When you choose NOT to reply, include `[NO_REPLY]` anywhere in your response.\n\n");
        }
    }

    // Scheduled tasks guidance
    out.push_str("## Scheduled Tasks\n\n");
    out.push_str("When using the `schedule` tool to create reminders:\n");
    out.push_str("- ALWAYS use double quotes (\") for the command string\n");
    out.push_str("- Example: `echo \"Time is up!\"`\n");
    out.push_str("- For Telegram chats, results can be auto-delivered when chat context is available\n\n");

    // Long-Term Autonomy
    out.push_str("## Long-Term Autonomy & Proactivity\n\n");
    out.push_str("1. **Goal Management:** Use `goal_add`, `goal_list`, and `goal_update` to manage long-term projects and objectives. \
        This allows you to track progress across different chat sessions and background tasks. \
        Check your goals periodically to ensure you're on track.\n");
    out.push_str("2. **Proactive Heartbeat:** You have a Heartbeat Engine that periodically triggers tasks from `HEARTBEAT.md`. \
        When you receive a message starting with `PROACTIVE TASK CHECK:`, it is an internal nudge to perform a recurring duty. \
        Respond to these by performing the task and reporting the outcome.\n\n");
}

fn inject_workspace_file(out: &mut String, workspace_dir: &str, filename: &str) {
    if let Some((_, path)) = open_workspace_file_guarded(workspace_dir, filename)
        && let Ok(content) = fs::read_to_string(path) {
            if content.trim().is_empty() {
                return;
            }
            // Inject the content directly without exposing the source filename
            if content.len() > BOOTSTRAP_MAX_CHARS {
                out.push_str(&content[..BOOTSTRAP_MAX_CHARS]);
                out.push_str("\n...[truncated]...\n");
            } else {
                out.push_str(&content);
            }
            out.push_str("\n\n");
        }
}

fn build_identity_section(out: &mut String, workspace_dir: &str, is_lean: bool) {
    out.push_str("## Project Context\n\n");
    out.push_str("The following workspace files define your identity, behavior, and context.\n\n");
    out.push_str("- **AGENTS.md**: Follow its operational guidance (startup routines, red-line constraints).\n");
    out.push_str("- **SOUL.md**: Embody its persona and tone. Avoid stiff, generic replies.\n");
    out.push_str("- **TOOLS.md**: User guidance for how to use external tools.\n\n");

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
        if is_lean {
            // Check for a compressed/summary version first
            let summary_name = filename.replace(".md", ".summary.md");
            if open_workspace_file_guarded(workspace_dir, &summary_name).is_some() {
                inject_workspace_file(out, workspace_dir, &summary_name);
                continue;
            }
            // Fallback: More aggressive truncation for lean mode
            inject_workspace_file_limited(out, workspace_dir, filename, 2000);
        } else {
            inject_workspace_file(out, workspace_dir, filename);
        }
    }

    // Memory file preference
    let mem_file = if open_workspace_file_guarded(workspace_dir, "MEMORY.md").is_some() {
        "MEMORY.md"
    } else {
        "memory.md"
    };

    if is_lean {
        inject_workspace_file_limited(out, workspace_dir, mem_file, 4000);
    } else {
        inject_workspace_file(out, workspace_dir, mem_file);
    }
}

fn inject_workspace_file_limited(
    out: &mut String,
    workspace_dir: &str,
    filename: &str,
    limit: usize,
) {
    if let Some((_, path)) = open_workspace_file_guarded(workspace_dir, filename)
        && let Ok(content) = fs::read_to_string(path) {
            if content.trim().is_empty() {
                return;
            }
            if content.len() > limit {
                out.push_str(&content[..limit]);
                out.push_str("\n...[truncated]...\n");
            } else {
                out.push_str(&content);
            }
            out.push_str("\n\n");
        }
}

fn build_tools_section(
    out: &mut String,
    tools: &[Arc<dyn Tool>],
    use_native_tools: bool,
    is_lean: bool,
) {
    out.push_str("## Tools\n\n");
    for tool in tools {
        if is_lean {
            // Minimal description for lean mode
            out.push_str(&format!("- **{}**: {}\n", tool.name(), tool.description()));
        } else {
            out.push_str(&format!(
                "- **{}**: {}\n  Parameters: `{}`\n",
                tool.name(),
                tool.description(),
                tool.parameters_json()
            ));
        }
    }

    // Only add tool calling instructions for non-native tool providers
    if !use_native_tools && !tools.is_empty() {
        out.push_str("\n## Tool Calling Instructions\n\n");
        if is_lean {
            out.push_str("Output a tool call block:\n\n");
            out.push_str("<tool_call>{\"name\": \"tool_name\", \"arguments\": {}}</tool_call>\n\n");
        } else {
            out.push_str(
                "To call a tool, output a tool call block in the following XML format:\n\n",
            );
            out.push_str("<tool_call>{\"name\": \"tool_name\", \"arguments\": {\"param1\": \"value1\", \"param2\": \"value2\"}}</tool_call>\n\n");
            out.push_str("Important:\n");
            out.push_str("- The tool call must be valid JSON inside the XML tags\n");
            out.push_str("- The 'name' field must match one of the available tools listed above\n");
            out.push_str("- The 'arguments' object must match the tool's parameter schema\n");
            out.push_str("- You can include multiple tool calls in your response\n\n");
        }
    }

    out.push('\n');
}

fn append_channel_attachments_section(out: &mut String) {
    out.push_str("## Channel Attachments\n\n");
    out.push_str("- On marker-aware channels (for example Telegram), you can send real attachments by emitting markers in your final reply.\n");
    out.push_str("- File/document: `[FILE:/absolute/path/to/file.ext]` or `[DOCUMENT:/absolute/path/to/file.ext]`\n");
    out.push_str("- Image/video/audio/voice: `[IMAGE:/abs/path]`, `[VIDEO:/abs/path]`, `[AUDIO:/abs/path]`, `[VOICE:/abs/path]`\n");
    out.push_str("- If user gives `~/...`, expand it to the absolute home path before sending.\n");
    out.push_str(
        "- Do not claim attachment sending is unavailable when these markers are supported.\n\n",
    );

    out.push_str("## Channel Choices\n\n");
    out.push_str("- On supported channels (for example Telegram when enabled), append `<nc_choices>...</nc_choices>` at the end of the final reply to render short button choices when you are asking the user to choose among short options.\n");
    out.push_str("- Always keep the normal visible question text before the choices block.\n");
    out.push_str(
        "- Use choices only for short mutually exclusive branches (for example yes/no or A/B).\n",
    );
    out.push_str(
        "- Do not use choices for long lists, open-ended prompts, or complex multi-step forms.\n",
    );
    out.push_str("- If you ask the user to pick one of 2-4 short explicit options (for example yes/no/cancel, A/B, or quoted command replies), you MUST append a choices block unless the user explicitly asked for plain text only.\n");
    out.push_str("- If you present a numbered or bulleted list of 2-4 mutually exclusive reply options, include matching choices for those same options.\n");
    out.push_str(
        "- The JSON must be valid and use `{\"v\":1,\"options\":[...]}` with 2-6 options.\n",
    );
    out.push_str("- Each option must include `id` and `label`; `submit_text` is optional (if omitted, label is used as submit text).\n");
    out.push_str("- `id` must be lowercase and contain only `a-z`, `0-9`, `_`, `-` (example: `yes`, `no`, `later_10m`).\n");
    out.push_str("- Example: `<nc_choices>{\"v\":1,\"options\":[{\"id\":\"yes\",\"label\":\"Yes\",\"submit_text\":\"Yes\"},{\"id\":\"no\",\"label\":\"No\"}]}</nc_choices>`\n\n");
}
