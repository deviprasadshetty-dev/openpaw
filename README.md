# 🐾 OpenPaw

<div align="center">
  <img src="image.png" alt="OpenPaw Mascot" width="300">
  <br/>
  <i>The purr-fectly autonomous, Rust-based AI Agent Runtime.</i>
</div>

---

OpenPaw is a high-performance, unapologetically modern AI Agent Runtime inspired by OpenClaw. It provides a robust, lightweight foundation for building autonomous agents that can interact with the world through a deeply integrated suite of tools. 

Whether it's reading your terminal, surfing the web with full visual context, or remembering what you said three weeks ago, OpenPaw is built to be your most reliable digital companion.

## ✨ Why OpenPaw?
* **Lightning Fast:** Written in Rust 🦀 for maximum performance, minimal memory footprint, and thread-safe concurrency.
* **Fully Autonomous:** Give it a task, and watch it figure out the terminal commands, file edits, and web searches needed to complete it.
* **Contextually Aware:** Automatically manages token limits and trims history so your agent never loses its train of thought.

## 🚀 Features & Superpowers

### 🧠 Core Capabilities
* **Persistent Memory:** Built-in SQLite-based memory with Full-Text Search (FTS5). OpenPaw remembers past conversations, facts, and preferences effortlessly.
* **Context Management:** Smart, automatic context window tracking.
* **Multi-Provider Support:** Plug-and-play compatibility with OpenAI, Anthropic, Gemini, and OpenRouter APIs.

### 🛠️ The Toolbelt
OpenPaw comes claws-out with a suite of powerful, native tools:
* **Advanced Web Browsing (`browser_use` integration):** *[NEW]* A fully autonomous, DOM-aware browser engine. It auto-detects Chrome, Edge, or Brave, launches isolated profiles, understands simplified DOM trees, and can click, type, scroll, and capture screenshots just like a human. 
* **Seamless Shell Access:** `shell` tool for executing terminal commands with built-in timeout and output truncation safety.
* **Local File System:** `file_read`, `file_write`, `file_edit`, `file_append` (sandboxed to your workspace directory).
* **Git Integration:** Native tools to manage repositories and understand diffs.
* **Web Search & Fetch:** Native DuckDuckGo / SearXNG search and HTTP fetching.
* **Composio Integration:** Connect to 1000+ apps (GitHub, Slack, Discord, etc.) via the `composio` tool.
* **Hardware & IoT:** I2C, SPI, and hardware memory info tools built right in.

### 📡 Communication Channels
* **Telegram:** Full bi-directional integration for interacting with your agent on the go.
* **CLI:** Chat with your agent directly from the terminal.
* **Gateway Server:** HTTP endpoints for external webhooks and integrations.

---

## 🛠️ Build & Setup

### Prerequisites
- Rust (latest stable)
- SQLite (bundled with `rusqlite`, no external install usually needed)
- A Chromium-family browser (Chrome, Edge, or Brave) for the web automation tools.

### Installation

1. Clone the repository:
   ```bash
   git clone https://github.com/your-username/openpaw.git
   cd openpaw
   ```

2. Configure your API keys (copy the template and edit it):
   ```bash
   cp config.json my_config.json
   ```
   *Required keys:*
   - `openai` API Key (or your preferred provider)
   - `telegram` Bot Token (if using the Telegram channel)

3. Build the project:
   ```bash
   cargo build --release
   ```

---

## 📁 Workspace Setup

OpenPaw uses a **workspace directory** to store your agent's identity, memory, and configuration files securely. The workspace is completely isolated from the source code.

### How It Works
- **Current Directory = Workspace**: By default, OpenPaw uses the directory you run the command from as the workspace.
- **Template files** (in `src/workspace_templates/`) are compile-time defaults used to initialize new workspaces.
- **Runtime files** (`AGENTS.md`, `SOUL.md`, etc.) are read from your workspace directory at runtime to define your agent's unique personality.

### Creating a Workspace (Interactive)

**Step 1: Install OpenPaw**
```bash
cd /path/to/openpaw
cargo install --path .
```

**Step 2: Create Your Workspace**
```bash
mkdir ~/my-agent-workspace
cd ~/my-agent-workspace

# Run interactive onboarding
openpaw onboard
```

You'll be prompted for:
1. **AI Provider** (OpenAI, Anthropic, Gemini, OpenRouter)
2. **API Key**
3. **Agent Name** (e.g., "Nova", "Whiskers")
4. **Your Name** & **Timezone**
5. **Telegram Details** (optional)

*OpenPaw will then scaffold your workspace with configuration and memory files.*

---

## 🏃 Usage

### Run the Agent Daemon
Starts the main agent process. It connects to configured channels (like Telegram) and listens for messages.
```bash
cd ~/my-agent-workspace
openpaw agent
```
*(Optionally specify a config file: `openpaw --config my_config.json agent`)*

### One-Shot Interaction
Pounce on a single task directly from the command line:
```bash
openpaw agent --message "Search the web for Rust tutorials, read the top result, and summarize it for me."
```

### Gateway Server
Start the HTTP gateway server for external integrations:
```bash
openpaw gateway
```

---

## 🛡️ Security & Privacy
OpenPaw implements strict path security. By default, file system tools are heavily restricted to your defined workspace directory. The new browser tools also utilize strictly isolated temporary browser profiles, meaning OpenPaw never touches or accesses your personal browser cookies, history, or extensions.
