# OpenPaw Web UI

A lightweight, modern web interface for managing your OpenPaw configuration and chatting with your AI agent.

## Features

- **Config Editor**: Visual form to manage all OpenPaw settings
  - AI Provider configuration with API key management
  - Memory backend settings
  - Channel configuration (Telegram, etc.)
  - HTTP request and browser automation toggles
  - Composio integration settings

- **Chat Interface**: Direct communication channel with your agent
  - Real-time messaging (WebSocket support coming soon)
  - Connection status indicator
  - Message history

## Usage

### Starting the Web UI

Run the OpenPaw gateway:

```bash
openpaw gateway
```

Or if running from source:

```bash
cargo run -- gateway
```

The Web UI will be available at: **http://127.0.0.1:3000**

### Configuration Sections

1. **AI Provider**: Set your default provider, model, and API keys
2. **Memory**: Configure memory backend (SQLite/Markdown/None) and embedding models
3. **Channels**: Set up Telegram bot tokens and allowed users
4. **HTTP Request**: Enable/disable web search and choose search provider
5. **Browser**: Toggle browser automation capabilities
6. **Composio**: Configure external app integrations

### API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/` | GET | Web UI (HTML/CSS/JS) |
| `/api/health` | GET | Health check |
| `/api/config` | GET | Get current configuration |
| `/api/config` | POST | Save configuration |
| `/api/chat` | POST | Send a chat message (HTTP fallback) |
| `/ws/chat` | GET | WebSocket chat (placeholder) |

## File Structure

```
static/
├── index.html    # Main HTML layout
├── styles.css    # Modern, responsive styling
└── app.js        # Client-side logic
```

## Technical Details

- **Frontend**: Vanilla HTML5, CSS3, JavaScript (no frameworks)
- **Backend**: Rust with Axum web framework
- **Styling**: Modern, responsive design with CSS variables
- **State Management**: In-memory config with automatic persistence

## Future Enhancements

- [ ] Full WebSocket chat integration with agent router
- [ ] Real-time streaming responses
- [ ] Markdown rendering in chat messages
- [ ] Session management and history
- [ ] Dark/Light theme toggle
- [ ] Multi-channel support in chat
- [ ] Config validation and error highlighting

## Security Notes

- API keys are stored in plain text in `config.json` - ensure proper file permissions
- By default, the gateway binds to `127.0.0.1` only
- To expose publicly, set `gateway.allow_public_bind = true` in config (not recommended without additional security)
