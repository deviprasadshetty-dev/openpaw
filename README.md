# OpenPaw

OpenPaw is a Rust-based, high-performance AI Agent Runtime inspired by OpenClaw. It provides a robust foundation for building autonomous agents that can interact with the world through various channels and tools.

## 🚀 Features & Powers

### Core Capabilities
*   **Persistent Memory:** Built-in SQLite-based memory with Full-Text Search (FTS5) for recalling past conversations and facts.
*   **Context Management:** Automatic context window tracking and history trimming to ensure agents stay within model token limits.
*   **Tool System:** Extensible tool interface for giving agents capabilities like file access, web search, and more.
*   **Multi-Provider Support:** Compatible with OpenAI, Anthropic, and OpenRouter APIs.

### Available Tools
OpenPaw comes with a suite of powerful tools out of the box:
*   **File System:** `file_read`, `file_write`, `file_edit`, `file_append` (with path security allowlists).
*   **Web Search:** `web_search` using DuckDuckGo (native) or SearXNG.
*   **Web Browsing:** `http_request` for API interactions and fetching content.
*   **System Integration:** `browser` tool for launching URLs in the user's default browser.
*   **Composio Integration:** Connect to 1000+ apps (GitHub, Slack, etc.) via the `composio` tool.

### Channels
*   **Telegram:** Full bi-directional integration with Telegram bots.
*   **CLI:** Interact with the agent directly from the terminal.

## 🛠️ Build & Setup

### Prerequisites
-   Rust (latest stable)
-   SQLite (bundled with `rusqlite`, no external install usually needed)

### Installation

1.  Clone the repository (if you haven't already).
2.  Navigate to the project directory:
    ```bash
    cd openpaw
    ```
3.  Configure your API keys:
    Copy the template and edit it:
    ```bash
    cp config.json my_config.json
    # Edit my_config.json with your favorite editor
    ```
    
    *Required keys:*
    -   `openai` API Key (or other provider)
    -   `telegram` Bot Token (if using Telegram)

4.  Build the project:
    ```bash
    cargo build --release
    ```

## 🏃 Usage

### Run the Agent Daemon
This starts the main agent process. It will connect to configured channels (like Telegram) and listen for messages.

```bash
cargo run --release -- agent
```

By default, it looks for `config.json` in the current directory. You can specify a custom config file:

```bash
cargo run --release -- --config my_config.json agent
```

### One-Shot Interaction
Send a single message to the agent from the command line and exit. Useful for quick tasks or testing tools.

```bash
cargo run --release -- agent --message "Search the web for Rust tutorials and save the top 3 links to rust_links.txt"
```

### Gateway Server
Start the HTTP gateway server for external integrations or webhooks.

```bash
cargo run --release -- gateway
```

## 🛡️ Security
OpenPaw implements path security to restrict file system access to the workspace directory. Ensure you review the `allowed_paths` configuration in the code if modifying tool permissions.
