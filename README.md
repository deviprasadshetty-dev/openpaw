# OpenPaw

OpenPaw is a Rust-native autonomous agent runtime for local work, messaging channels, long-running background tasks, memory, scheduling, tools, and multimodal workflows.

It is designed to be a general-purpose personal agent: it can talk in chat, inspect and edit files, run commands, search the web, use browser/vision tools, schedule follow-ups, spawn background workers, remember useful context, and keep making progress without turning every request into a permission loop.

## Highlights

- **General-purpose autonomy**: the runtime prompt is tuned to investigate first, act on safe/reversible work, verify outcomes, and ask only for genuinely risky external actions.
- **Background plans and subagents**: large work can be split into dependency-aware plans or individual background tasks, with final integration review.
- **Task visibility**: `task_status`, `task_list`, and `plan_status` expose queued, running, completed, failed, and cancelled background work.
- **Telegram-first mobile workflow**: Telegram supports polling or webhooks, streaming message edits, typing indicators, reply threading, inline choice buttons, attachments, voice/photo/document/video intake, forum topic routing, edited messages, stickers, locations, and contacts.
- **Persistent memory and goals**: SQLite/Markdown-backed memory, active goals, session search, memory hygiene, and background learning help the agent carry context across sessions.
- **Scheduling and proactive checks**: cron jobs, one-shot reminders, heartbeat tasks, and proactive Telegram notifications are built in.
- **Rich toolbelt**: file operations, shell, web search/fetch, browser automation, vision, hardware tools, skills, MCP tools, Composio, OpenCode CLI, and more.
- **Provider flexibility**: Gemini, Gemini CLI, OpenAI, Anthropic, OpenRouter, Kilo.ai, OpenCode, Ollama, and LM Studio are supported.

## What OpenPaw Can Do

### Autonomous Work

OpenPaw can take an open-ended request and keep working through the loop:

1. Gather context from files, memory, tools, browser, search, or prior sessions.
2. Make a lightweight plan.
3. Execute with tools.
4. Verify the result.
5. Report what changed and what remains.

For big tasks, OpenPaw can move work into the background instead of slowly streaming every step in the foreground. It can create a plan, launch subagents, wait for them, cancel timed-out work, run a final review, and report the integrated result back to the chat.

Useful tools:

- `spawn`: launch one background subagent.
- `delegate`: launch a named agent profile.
- `plan_create`: run a dependency-aware background plan.
- `plan_status`: inspect a running or completed plan.
- `task_status`: inspect one background task.
- `task_list`: list queued/running tasks, optionally including completed work.
- `task_cancel`: cancel queued or running work.

### Telegram

OpenPaw's Telegram adapter supports:

- Long polling and webhook mode.
- Direct messages, groups, and supergroup forum topics.
- Per-topic session keys and replies using `message_thread_id`.
- Typing indicators during processing.
- Streaming responses by editing an in-progress Telegram message.
- Reply-to behavior for normal responses and errors.
- Inline button choices via `<nc_choices>...</nc_choices>`.
- Inbound photos, documents, voice notes, audio, videos, media groups, stickers, locations, contacts, and edited messages.
- Outbound text, photos, documents, audio, video, and inline base64 image attachments.
- Group allowlists, group behavior rules, and `[NO_REPLY]` support.
- Proactive Telegram notifications with `notify_telegram`.

### Memory, Goals, and Learning

OpenPaw has durable context instead of relying only on the current chat window:

- Runtime memory with SQLite, Markdown, PostgreSQL, or LRU-style backends.
- `memory_store`, `memory_recall`, `memory_list`, and `memory_forget`.
- Active goals via `goal_add`, `goal_list`, and `goal_update`.
- Cross-session search for previous decisions and workflows.
- Memory hygiene that consolidates stale entries.
- Dream/background learning that extracts preferences and reusable patterns.
- Skill creation and maintenance for repeatable workflows.

### Scheduling

OpenPaw can run tasks later or repeatedly:

- Cron expressions.
- One-shot delays.
- Shell jobs.
- Isolated agent jobs.
- Run history.
- Delivery on success, error, always, or never.
- Heartbeat checks defined in `HEARTBEAT.md`.

Examples:

```bash
openpaw cron add "0 9 * * *" --message "Check my priorities for today"
openpaw cron add --delay "20m" --message "Remind me to check the oven"
```

### Tools

OpenPaw includes tools for:

- File read/write/edit/append/delete.
- Shell commands.
- Git operations.
- Web search and URL fetch.
- Browser automation and screenshots.
- Vision and multimodal file analysis.
- Hardware info, hardware memory, I2C, SPI, and serial-oriented workflows.
- Skill search/install/list/manage/view.
- MCP server tools.
- Composio tools.
- OpenCode CLI delegation.
- Telegram notifications.
- Approvals for sensitive subagent actions.
- Inter-agent mailbox communication.

## Install

### Windows PowerShell

```powershell
powershell -ExecutionPolicy ByPass -Command "irm https://raw.githubusercontent.com/deviprasadshetty-dev/openpaw/main/install.ps1 | iex"
```

### Linux/macOS

```bash
curl -sSf https://raw.githubusercontent.com/deviprasadshetty-dev/openpaw/main/install.sh | bash
```

### Manual Build

```bash
git clone https://github.com/deviprasadshetty-dev/openpaw.git
cd openpaw
cargo build --release
cargo install --path .
```

## Setup

Run the onboarding wizard:

```bash
openpaw onboard
```

The wizard configures:

- AI provider and model.
- Memory backend.
- Voice transcription.
- Telegram bot access.
- Composio integrations.
- Web search.

You can re-run onboarding later; it preserves existing settings where possible.

## Run

Start the persistent agent daemon:

```bash
openpaw agent
```

Send a one-shot task from the terminal:

```bash
openpaw agent --message "Check this project for failing tests and summarize what needs fixing"
```

Run with Telegram configured, then message your bot directly.

## Configuration Notes

OpenPaw creates workspace files such as:

- `SOUL.md`: assistant identity and behavior.
- `USER.md`: user preferences.
- `MEMORY.md`: environment and durable project notes.
- `AGENTS.md`: workspace operating rules.
- `HEARTBEAT.md`: proactive recurring checks.
- `BOOTSTRAP.md`: first-run initialization notes.

The generated agent prompt treats those files as private implementation context and avoids exposing their names or raw contents in normal replies.

## Security Model

OpenPaw defaults to useful autonomy, but keeps boundaries:

- Act freely on safe, reversible, and clearly requested local work.
- Ask before destructive operations outside the workspace.
- Ask before external messages/posts/emails to real people.
- Ask before actions with cost, compliance, legal, or major resource impact.
- Keep private data private.
- Preserve approval workflows instead of bypassing them.

## Development

Common commands:

```bash
cargo fmt
cargo check
cargo test
```

The test suite currently covers provider parsing, Telegram handling, memory parsing, token estimation, secrets, utility behavior, routing hints, and agent response cleanup.

## Project Status

OpenPaw is moving toward a proactive, multi-channel, general-purpose autonomous agent runtime. Recent work added:

- Big-task background routing.
- Dependency-aware plan execution with final review.
- Task and plan status tools.
- Timeout cancellation for spawned plan tasks.
- Subagent memory and goal access.
- Cron agent jobs through live sessions.
- Heartbeat interval fixes.
- Telegram forum topic routing and richer Telegram update handling.
- More general-purpose autonomy prompt tuning.

