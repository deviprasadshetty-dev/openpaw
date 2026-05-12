# OpenPaw

OpenPaw is a personal autonomous AI agent that runs on your machine and helps you get real work done across chat, files, the web, tools, schedules, and background tasks.

It is built in Rust for people who want an assistant that can do more than answer questions. OpenPaw can investigate, act, remember, follow up, and keep working on larger tasks without forcing every step through the foreground chat.

## Why OpenPaw

Most assistants are reactive. OpenPaw is designed to be useful in a more practical way:

- It can work from Telegram, the terminal, WhatsApp, email, or HTTP.
- It can use your local workspace, shell, browser, memory, and tools.
- It can break large requests into background work and report back when done.
- It can remember preferences and project context across sessions.
- It can schedule reminders, recurring checks, and proactive follow-ups.
- It can handle text, images, documents, audio, video, and voice notes.
- It can stay careful around destructive, external, or sensitive actions.

## Key Features

### Autonomous Task Handling

OpenPaw can take a broad request, gather context, use tools, verify the result, and give you a clear summary. For larger work, it can move long-running pieces into the background so the main conversation stays usable.

### Telegram-Ready

Telegram is a first-class channel. OpenPaw supports direct chats, groups, forum topics, replies, streaming message updates, inline buttons, photos, documents, audio, video, voice notes, stickers, shared locations, contacts, and edited messages.

### Memory and Continuity

OpenPaw can keep durable memory about preferences, projects, workflows, and useful facts. It can also search prior sessions when you refer to something from the past.

### Scheduling and Proactivity

Use OpenPaw for reminders, recurring checks, background tasks, and periodic heartbeats. It can notify you when something is due or when a background job finishes.

### Multimodal Work

OpenPaw can inspect images, videos, audio, PDFs, screenshots, and local files. It can also send generated or existing files back through supported chat channels.

### Local Tools

OpenPaw can work with your file system, shell, Git, browser, web search, skills, MCP tools, hardware interfaces, and external app integrations.

### Flexible Models

OpenPaw supports multiple AI providers, including Gemini, Gemini CLI, OpenAI, Anthropic, OpenRouter, Kilo.ai, OpenCode, Ollama, and LM Studio.

## Install

### Windows

```powershell
powershell -ExecutionPolicy ByPass -Command "irm https://raw.githubusercontent.com/deviprasadshetty-dev/openpaw/main/install.ps1 | iex"
```

### Linux/macOS

```bash
curl -sSf https://raw.githubusercontent.com/deviprasadshetty-dev/openpaw/main/install.sh | bash
```

### Build From Source

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

The wizard helps configure your model provider, memory, Telegram, voice, search, and integrations.

## Run

Start OpenPaw as a persistent agent:

```bash
openpaw agent
```

Send a one-shot task:

```bash
openpaw agent --message "Check this project and tell me what needs attention"
```

## Common Uses

- Ask it to investigate a project or folder.
- Send voice notes from Telegram.
- Drop in screenshots, documents, or videos for analysis.
- Schedule reminders and recurring checks.
- Let it work on large multi-step tasks in the background.
- Use it as a mobile command center for your local machine.
- Build up reusable skills and memory over time.

## Safety

OpenPaw is built to act, but not recklessly.

It can freely handle safe local work, but should pause before destructive actions outside the workspace, external messages to real people, high-cost operations, or anything sensitive.

## Development

```bash
cargo fmt
cargo check
cargo test
```

## Status

OpenPaw is actively evolving toward a practical, general-purpose autonomous assistant: local-first, multi-channel, memory-aware, tool-using, and capable of background work.

