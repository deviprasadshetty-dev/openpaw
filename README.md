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

Whether it's prowling through your file system, sniffing out data in technical datasheets, or pouncing on complex web automation tasks, OpenPaw is designed to be your most loyal—and technically superior—digital companion.

## 🚀 Why OpenPaw?
*   **Rust-Native Instincts 🦀:** Zero-cost abstractions and thread-safe concurrency for "purr-formance" that never lags.
*   **Claws-Out Automation:** Native support for I2C, SPI, and Serial communication to interact with the physical world.
*   **OpenClaw Heritage:** Fully compatible with the "Skills" philosophy, offering a familiar but "sharper" experience for OpenClaw enthusiasts.
*   **Hardware-Aware Senses:** A specialized RAG system that understands the "anatomy" of hardware (datasheets, pin-aliases, and board types).

## ✨ The Agent's Instincts (Features)

### 🧠 Sophisticated Orchestration
*   **7-Tier Territory Routing:** Advanced logic that routes messages based on Peer, Guild, Team, Account, or Channel constraints. Your agent always knows its place.
*   **The Whisker-Thin Bus:** A high-throughput internal message bus (via `crossbeam-channel`) that orchestrates silent, deadly-efficient communication between modules.
*   **Persistent Memory:** SQLite-backed long-term memory with Full-Text Search (FTS5). OpenPaw remembers your preferences like a cat remembers its favorite sunny spot.

### 🛠️ Sharpening the Claws (The Toolbelt)
*   **MCP Host Implementation:** OpenPaw hosts and orchestrates Model Context Protocol (MCP) servers natively, expanding its "territory" to thousands of standardized tools.
*   **Hardware-Aware RAG:** A specialized sensory system for technical documentation. It parses markdown pin-aliases and provides the exact board-specific context needed for hardware hacks.
*   **Deep Web Prowling:** Powered by `browser-use`, OpenPaw navigates the web with human-like precision, using isolated profiles to keep your digital "scent" hidden.
*   **SkillForge Ecosystem:** Automatically scout and integrate community "Skills" from GitHub. Compatible with the NullClaw and OpenClaw ecosystems.

### 🔌 Multimodal Senses
*   **Hardware Gateway:** Native drivers for Serial (ACM/USB), I2C, and SPI. Control real-world hardware as easily as playing with a laser pointer.
*   **Multimodal Ears (Groq/Whisper):** Bi-directional voice support. OpenPaw can "hear" Telegram voice notes and transcribe them instantly using ultra-low latency STT.
*   **Multi-Channel Prowling:** Robust adapters for Telegram, CLI, and a high-performance HTTP Gateway for custom webhooks.

---

## 🛠️ Setting up the Litter Box (Build & Setup)

### Prerequisites
- Rust (latest stable)
- SQLite (bundled)
- A modern browser (Chrome/Edge/Brave) for web automation.

### Installation

1.  **Clone & Build:**
    ```bash
    git clone https://github.com/your-username/openpaw.git
    cd openpaw
    cargo build --release
    ```

2.  **Onboarding (The "First Meow"):**
    OpenPaw features an interactive onboarding process to scaffold your workspace:
    ```bash
    cargo run -- release onboard
    ```
    *This will help you configure your AI providers (OpenAI, Anthropic, Gemini, etc.), set up your agent's soul (`SOUL.md`), and link your communication channels.*

---

## 🏃 Deployment Territories

### 📡 The Resident Daemon
Run OpenPaw as a persistent background service to handle incoming calls from Telegram or webhooks:
```bash
openpaw agent
```

### ⚡ Quick Pounces (One-Shot)
Execute complex tasks directly from your terminal:
```bash
openpaw agent --message "Sniff out the CPU temperature and let me know if it's getting too hot."
```

---

## 🛡️ Security & Territory Isolation
OpenPaw is fiercely protective of its territory. File system access is strictly sandboxed. Web browsing occurs in ephemeral, isolated Chromium containers. All hardware interactions are subject to strict path-based permission checks.

---

<div align="center">
  <i>Developed with ❤️ for the Rust, AI, and Cat communities.</i>
  <br/>
  <b>May your agents always land on all fours.</b>
</div>
