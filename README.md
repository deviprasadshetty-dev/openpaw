# 🐾 OpenPaw

<div align="center">
  <img src="image.png" alt="OpenPaw Mascot" width="300">
  <br/>
  <i><b>The Purr-fectly Powerful Cat AI Assistant</b></i>
  <br/>
  <i>Inspired by OpenClaw. Engineered in Rust. Built to land on all fours.</i>
</div>

---

OpenPaw is a high-performance, unapologetically modular AI Agent Runtime. Inspired by the legendary **OpenClaw**, OpenPaw is a spiritual successor written from the ground up in Rust for those who need a domestic AI that's as fast as a feline and twice as sharp.

Whether it's prowling through your file system, or sniffing out data in technical datasheets, OpenPaw is designed to be your most loyal—and technically superior—digital companion.

## 🚀 Why OpenPaw?
*   **Rust-Native Instincts 🦀:** Zero-cost abstractions and thread-safe concurrency for "purr-formance" that never lags.
*   **Claws-Out Automation:** Native support for I2C, SPI, and Serial communication to interact with the physical world.
*   **OpenClaw Heritage:** Fully compatible with the "Skills" philosophy, offering a familiar but "sharper" experience for OpenClaw enthusiasts.
*   **Hardware-Aware Senses:** A specialized RAG system that understands the "anatomy" of hardware (datasheets, pin-aliases, and board types).
*   **Cost Optimized:** Prompt caching (75% reduction), response caching, tool result caching, and cheap model routing for greetings — built for sustained use.
*   **Self-Improving:** Dream Sequence autonomous learning, skill nudges, and dialectic user modeling make OpenPaw smarter over time.

## ✨ The Agent's Instincts (Features)

### 🤖 AI Providers

Connect to the best AI models through a unified interface. OpenPaw supports **9 providers out of the box**:

| Provider | Free Tier | Notes |
|---|---|---|
| **Gemini** | ✅ 15 req/min, 1M tokens/day | Google AI Studio key |
| **Gemini CLI** | ✅ No key needed | Reuses your existing `gemini` CLI OAuth session |
| **OpenAI** | ❌ | GPT-4o, o1, o3 |
| **Anthropic** | ❌ | Claude 3.5 / 3.7 |
| **OpenRouter** | ✅ 25+ free models | 200+ models via one key — **auto-detected** |
| **Kilo.ai** | ✅ Free + 200+ paid | Gateway to hundreds of models — **auto-detected** |
| **OpenCode** | ✅ | OpenCode Zen free models |
| **Ollama** | ✅ Fully local | No key needed, runs on your machine |
| **LM Studio** | ✅ Fully local | No key needed, runs on your machine (port 1234) |

#### 🆓 Smart Free Model Detection
When you choose **OpenRouter** or **Kilo.ai** during setup, OpenPaw automatically:
- Fetches the full model list live from the provider's API
- **Filters for genuinely free models** (`pricing.prompt == 0`, `pricing.completion == 0`)
- For OpenRouter, enforces a **minimum 8,192-token context window** — filters out unusable tiny models, keeping only 32K–512K context ones useful for agents
- Presents a ranked list (largest context first) with a recommended default
- Saves remaining free models as **automatic runtime fallbacks**

#### 🔄 Automatic Runtime Fallback (Kilo.ai)
If your selected model fails (rate-limited, overloaded, unavailable), OpenPaw silently retries with the next free model in your fallback list — no interruption, no errors. Logged as warnings so you can see what happened.

```
WARN [kilocode] Model 'minimax/minimax-m2.1:free' failed: 429. Trying fallbacks…
WARN [kilocode] Succeeded with fallback model 'arcee-ai/trinity-large-preview:free'
```

#### 🎯 Sticky Model Selection & Config Integrity
OpenPaw's onboarding wizard now treats your model choice as first-class configuration:
- **Provider-level model persistence** — the selected model is written inside `models.providers.<name>.model` in `config.json`, right alongside `api_key` and `base_url`. Every provider carries its own complete identity.
- **Sticky defaults on re-edit** — if you re-run `openpaw onboard`, the wizard fetches the live model list and **pre-selects the exact model you already have**, instead of blindly jumping to index 0 and risking an accidental switch.
- **First-setup smarts** — on a fresh install, OpenRouter and Kilo.ai still recommend the highest-context free model as the default; on subsequent edits, your personal choice is preserved faithfully.

---

### 🧠 Sophisticated Orchestration & Intelligence
*   **Contextual Intelligence 🧠**: OpenPaw is aware of "now" with native UTC date and time injection, and features intelligent skill discovery for lazy-loading capabilities.
*   **Social & Channel Awareness**: Specialized logic for Telegram group chats (using `[NO_REPLY]` markers) and strict guidance for scheduled tasks to minimize execution errors.
*   **7-Tier Territory Routing:** Advanced logic that routes messages based on Peer, Guild, Team, Account, or Channel constraints. Your agent always knows its place.
*   **The Whisker-Thin Bus:** A high-throughput internal message bus (via `crossbeam-channel`) that orchestrates silent, deadly-efficient communication between modules.
*   **Persistent Memory:** Multi-backend memory system (SQLite with FTS5, Markdown, PostgreSQL, LRU). OpenPaw remembers your preferences like a cat remembers its favorite sunny spot.
*   **Dream Sequence 🌙:** Autonomous learning during idle time (15+ min). Reviews memories, extracts core learnings, deletes obsolete data, and suggests NEW skills — all via LLM-based consolidation.
*   **Prompt Caching (75% Cost Reduction):** Anthropic Cache Control implementation with smart invalidation. Frozen snapshots preserve prefix cache for massive cost savings.
*   **Context Compression:** Pre-flight token counting, dynamic max tokens adjustment, and trivial follow-up detection ("yes"/"no"/"ok") for efficient context usage.
*   **Follow-Through Guardrail:** Detects when the LLM promises action but doesn't call tools — injects nudges to execute promised actions and prevents infinite loops.
*   **Response & Tool Caching:** LLM response caching (1-hour TTL) and tool result caching (5-minute TTL) with circuit breaker (stops retrying tools failing 3+ times).
*   **Self-Learning Agent:** Skill nudges for repeated workflows, memory nudges for important info, and `DIALECTIC.md` user modeling that captures your preferences and style.

### 🛠️ Sharpening the Claws (The Toolbelt)
*   **40+ Built-in Tools:** Comprehensive toolset including file operations, shell commands, web search/fetch, memory management, skill operations, cron scheduling, messaging, browser automation, vision, and more.
*   **MCP Host Implementation**: OpenPaw hosts and orchestrates Model Context Protocol (MCP) servers natively, expanding its "territory" to thousands of standardized tools with JSON-RPC 2.0 support.
*   **Hardware-Aware RAG**: A specialized sensory system for technical documentation. It parses markdown pin-aliases, detects boards (Arduino, STM32, ESP32 via USB VID/PID), and provides exact board-specific context.
*   **Brave Search Integration**: High-quality web results via Brave's Search API. Returns rich, agent-friendly snippets for superior information gathering. Supports DuckDuckGo and Gemini search as fallbacks.
*   **Background Sub-agents**: OpenPaw can spawn and manage background workers (default concurrency: 4) for long-running tasks, allowing it to multi-task without blocking your main conversation.
*   **SkillForge Ecosystem**: Automatically scout and integrate community "Skills" from GitHub, ClawHub, and skills.sh. Compatible with NullClaw and OpenClaw ecosystems. Includes built-in `skill-creator` for building new skills.
*   **Composio Integration 🔗**: Connect 100+ external apps (Gmail, GitHub, Slack, Jira, and more) via Composio for enterprise-grade workflow automation.
*   **OpenCode CLI Bridge**: Optional `opencode_cli` tool lets OpenPaw invoke `opencode run` (including `--attach`) for a second coding agent pipeline when you need deeper code reasoning or different tool ecosystems.
*   **Cost Tracking 💰**: Real-time token usage and cost estimation across all providers. Tracks spend per session and over time.
*   **Approval Workflows:** Human-in-the-loop with `request_approval`/`approval_respond` tools. Configurable via `AgentMailbox` for inter-agent messaging.
*   **Workspace Templates:** Auto-creates `SOUL.md` (personality), `USER.md` (preferences), `MEMORY.md` (knowledge), `AGENTS.md` (instructions), `HEARTBEAT.md` (proactive tasks), and `BOOTSTRAP.md` (init).

### 🔌 Multimodal Senses
*   **Hardware Gateway**: Native drivers for Serial (ACM/USB), I2C, and SPI. Control real-world hardware as easily as playing with a laser pointer. Board auto-detection via USB VID/PID.
*   **Multimodal Ears (Groq/Whisper)**: Bi-directional voice support. OpenPaw can "hear" Telegram voice notes and transcribe them instantly using ultra-low latency STT.
*   **Multimodal Vision**: Process images (PNG, JPEG, GIF, BMP, WebP) and video (MP4, MPEG, MOV, WebM) with `[image:path]` and `[video:path]` marker syntax.
*   **Multi-Channel Prowling**: Robust adapters for **Telegram**, **CLI**, **WhatsApp Native**, and **Email** (IMAP/SMTP). 7-tier routing with per-peer/per-channel session management.
*   **Browser Automation**: Multiple backends — CDP (Chrome DevTools Protocol), Native WebDriver, and Computer Use (Anthropic-style). Auto-launch Chrome/Chromium with headless support.

---

## 🛠️ Setting up the Litter Box (Build & Setup)

### Prerequisites
- Rust (latest stable)
- SQLite (bundled)

### ⚡ One-Line Install
**Windows (PowerShell):**
```powershell
powershell -ExecutionPolicy ByPass -Command "irm https://raw.githubusercontent.com/deviprasadshetty-dev/openpaw/main/install.ps1 | iex"
```

**Linux/macOS:**
```bash
curl -sSf https://raw.githubusercontent.com/deviprasadshetty-dev/openpaw/main/install.sh | bash
```

### Installation (Manual)

1.  **Clone & Build:**
    ```bash
    git clone https://github.com/deviprasadshetty-dev/openpaw.git
    cd openpaw
    cargo build --release
    ```

2.  **Install Globally:**
    To use OpenPaw from anywhere in your terminal:
    ```bash
    cargo install --path .
    ```

3.  **Onboarding (The "First Meow"):**
    Run the interactive setup wizard — now with a modern color terminal UI:
    ```bash
    openpaw onboard
    ```

    The wizard walks you through **6 focused steps** — only things that go into `config.json`:

    | Step | What it configures |
    |---|---|
    | **1. AI Provider** | Provider, API key, model selection with live free-model fetch (OpenRouter/Kilo.ai) |
    | **2. Memory** | SQLite / Markdown / None + optional vector embeddings |
    | **3. Voice** | Groq Whisper transcription (free key at console.groq.com) |
    | **4. Telegram** | Bot token for mobile chat |
    | **5. Composio** | External app integrations (Gmail, GitHub, Slack…) |
    | **6. Web Search** | Brave Search API key for high-quality web results |

    No fluff — no questions about names or timezones that don't affect your agent's behaviour.

    > 💡 **Re-run anytime:** `openpaw onboard` is fully idempotent. It reads your existing `config.json`, pre-selects your current model and settings, and only asks about things you haven't configured yet.

---

## 🏃 Deployment Territories

### 📡 The Resident Daemon
Run OpenPaw as a persistent background service to handle incoming calls from Telegram, WhatsApp, Email, or webhooks:
```bash
openpaw agent
```

### ⚡ Quick Pounces (One-Shot)
Execute complex tasks directly from your terminal:
```bash
openpaw agent --message "Sniff out the CPU temperature and let me know if it's getting too hot."
```

### 📅 Cron & Scheduling
Built-in cron system with support for:
- **Cron expressions:** `0 9 * * *` (daily at 9am)
- **At timestamps:** One-time future execution
- **Every intervals:** Repeated execution (millisecond precision)
- **Job types:** Shell commands or isolated agent turns
- **Delivery modes:** None, Always, OnError, OnSuccess
- **Run history:** Tracks execution history with 1-second polling

```bash
openpaw cron add "0 9 * * *" --message "Good morning! Any tasks for today?"
```

### 🧩 OpenCode CLI Bridge (Optional)
Enable in your `config.json`:
```json
"opencode_cli": {
  "enabled": true,
  "binary": "opencode",
  "timeout_secs": 180,
  "max_output_bytes": 1000000,
  "attach_url": "http://127.0.0.1:4096"
}
```

Use with a warm OpenCode server for faster repeated calls:
```bash
opencode serve --port 4096
```

Suggested uses:
- Complex coding/refactoring and debugging
- Research synthesis and long-form summarization
- Planning (milestones, alternatives, decision matrices)
- Writing transformations (rewrite, tone shift, structured drafts)

### 🌐 Gateway Server
Expose OpenPaw via HTTP gateway with WebSocket support for real-time communication and tunneling capabilities.

---

## 🛡️ Security & Territory Isolation
OpenPaw is fiercely protective of its territory:
- **Secret Encryption:** ChaCha20Poly1305 encryption for API keys and sensitive config
- **File System Sandboxing:** Strict path-based permission checks for file operations
- **Channel Allowlists:** Control which channels can interact with your agent
- **Network Security:** CORS protection and allowed domain restrictions
- **Web Isolation:** Browsing occurs in ephemeral, isolated Chromium containers
- **Hardware Permissions:** All hardware interactions subject to strict path-based checks

---

<div align="center">
  <i>Developed with ❤️ for the Rust, AI, and Cat communities.</i>
  <br/>
  <b>May your agents always land on all fours.</b>
</div>
