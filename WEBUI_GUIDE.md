# OpenPaw Web UI - Quick Start Guide

## 🚀 Getting Started

### Step 1: Build OpenPaw (if not already built)

```bash
cargo build --release
```

### Step 2: Start the Agent (with Web UI)

```bash
# Using the installed binary
openpaw agent

# Or from source
cargo run --release -- agent
```

**That's it!** The Web UI now starts automatically with the agent.

### Step 3: Open Your Browser

Navigate to: **http://127.0.0.1:3000**

You should see the OpenPaw Control Panel with two tabs:
- **Config** - Manage your configuration
- **Chat** - Chat with your agent

---

## 📋 Config Tab Features

### AI Provider Settings
- Select your default AI provider (Gemini, OpenAI, Anthropic, etc.)
- Set the default model name
- Enter API keys for each provider you want to use

### Memory Settings
- Choose memory backend: SQLite, Markdown, or None
- Configure embedding model for vector search

### Channels
- Add your Telegram bot token from @BotFather
- Specify allowed usernames (comma-separated)

### HTTP Request
- Enable/disable web search capabilities
- Choose search provider (DuckDuckGo or Brave)

### Browser Automation
- Toggle browser automation on/off

### Composio Integration
- Enable Composio for external app integrations
- Add your Composio API key
- Set entity ID

---

## 💬 Chat Tab

The chat interface allows you to communicate directly with your OpenPaw agent.

**Current Status**: 
- ✅ HTTP chat endpoint available at `/api/chat`
- 🔜 WebSocket support coming soon for real-time streaming

---

## 🔧 Customization

### Change Port or Host

Edit your `config.json`:

```json
{
  "gateway": {
    "port": 3000,
    "host": "127.0.0.1"
  }
}
```

### Run Gateway Only (No Agent Channels)

If you only want the Web UI without running agent channels:

```bash
openpaw gateway
```

### Expose Publicly (Not Recommended Without Security)

```json
{
  "gateway": {
    "allow_public_bind": true
  }
}
```

⚠️ **Warning**: Only expose publicly if you've added proper authentication!

---

## 🎨 UI Features

- **Responsive Design**: Works on desktop and mobile
- **Modern Styling**: Clean, professional interface
- **Smooth Animations**: Polished user experience
- **Toast Notifications**: Visual feedback for actions
- **Connection Status**: Real-time indicator for chat

---

## 🛠️ Troubleshooting

### Web UI Not Loading

1. Check if the gateway started successfully
2. Verify port 3000 is not in use by another application
3. Check logs for any errors

### Config Not Saving

1. Ensure `config.json` has write permissions
2. Check that the config file path is correct
3. Look for error messages in the terminal

### Chat Not Connecting

1. The WebSocket endpoint is currently a placeholder
2. Use the HTTP endpoint `/api/chat` for now
3. Full WebSocket integration coming in future updates

---

## 📁 File Locations

- **Web UI Files**: `static/` directory
  - `index.html` - Main page
  - `styles.css` - Styling
  - `app.js` - Client logic
- **Config File**: `config.json` (in workspace or home directory)
- **Binary**: `target/release/openpaw`

---

## 🔮 Coming Soon

- Real-time WebSocket chat with streaming responses
- Markdown rendering in chat
- Conversation history
- Multiple chat sessions
- Config validation with helpful error messages
- Dark mode theme toggle

---

**Enjoy your new OpenPaw Web UI! 🐾**
