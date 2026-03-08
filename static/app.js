// OpenPaw Web UI - Main Application Logic

class OpenPawUI {
    constructor() {
        this.config = {};
        this.ws = null;
        this.isConnected = false;
        this.init();
    }

    init() {
        this.setupTabs();
        this.setupConfigForm();
        this.setupChat();
        this.loadConfig();
    }

    // Tab Navigation
    setupTabs() {
        const tabBtns = document.querySelectorAll('.tab-btn');
        tabBtns.forEach(btn => {
            btn.addEventListener('click', () => {
                const tabName = btn.dataset.tab;

                // Update buttons
                tabBtns.forEach(b => b.classList.remove('active'));
                btn.classList.add('active');

                // Update content
                document.querySelectorAll('.tab-content').forEach(content => {
                    content.classList.remove('active');
                });
                document.getElementById(`${tabName}-tab`).classList.add('active');
            });
        });
    }

    // Config Management
    setupConfigForm() {
        const form = document.getElementById('config-form');
        form.addEventListener('submit', (e) => this.saveConfig(e));

        document.getElementById('load-config').addEventListener('click', () => this.loadConfig());

        // Setup smooth scroll for navigation
        document.querySelectorAll('.config-nav a').forEach(link => {
            link.addEventListener('click', (e) => {
                e.preventDefault();
                const targetId = link.getAttribute('href').substring(1);
                const section = document.getElementById(targetId);
                section.scrollIntoView({ behavior: 'smooth', block: 'start' });

                // Update active state
                document.querySelectorAll('.config-nav a').forEach(l => l.classList.remove('active'));
                link.classList.add('active');
            });
        });
    }

    async loadConfig() {
        try {
            const response = await fetch('/api/config');
            if (!response.ok) throw new Error('Failed to load config');

            this.config = await response.json();
            this.populateForm(this.config);
            this.showToast('Config loaded successfully', 'success');
        } catch (error) {
            console.error('Error loading config:', error);
            this.showToast('Failed to load config', 'error');
        }
    }

    populateForm(config) {
        // Basic settings
        this.setFieldValue('default_provider', config.default_provider);
        this.setFieldValue('default_model', config.default_model || '');

        // Provider API keys
        if (config.models && config.models.providers) {
            const providers = config.models.providers;
            this.setFieldValue('providers.gemini.api_key', providers.gemini?.api_key || '');
            this.setFieldValue('providers.openai.api_key', providers.openai?.api_key || '');
            this.setFieldValue('providers.anthropic.api_key', providers.anthropic?.api_key || '');
            this.setFieldValue('providers.openrouter.api_key', providers.openrouter?.api_key || '');
            this.setFieldValue('providers.kilo.api_key', providers.kilo?.api_key || '');
        }

        // Memory
        if (config.memory) {
            this.setFieldValue('memory.backend', config.memory.backend || 'sqlite');
            this.setFieldValue('memory.embedding_model', config.memory.embedding_model || '');
        }

        // Channels - Telegram
        if (config.channels && config.channels.telegram && config.channels.telegram.length > 0) {
            const telegram = config.channels.telegram[0];
            this.setFieldValue('channels.telegram[0].bot_token', telegram.bot_token || '');
            this.setFieldValue('channels.telegram[0].allow_from', telegram.allow_from?.join(', ') || '');
        }

        // HTTP Request
        if (config.http_request) {
            this.setCheckbox('http_request.enabled', config.http_request.enabled || false);
            this.setFieldValue('http_request.search_provider', config.http_request.search_provider || 'duckduckgo');
        }

        // Browser
        if (config.browser) {
            this.setCheckbox('browser.enabled', config.browser.enabled !== false);
        }

        // Composio
        if (config.composio) {
            this.setCheckbox('composio.enabled', config.composio.enabled || false);
            this.setFieldValue('composio.api_key', config.composio.api_key || '');
            this.setFieldValue('composio.entity_id', config.composio.entity_id || 'default');
        }
    }

    setFieldValue(name, value) {
        const field = document.querySelector(`[name="${name}"]`);
        if (field) {
            field.value = value;
        }
    }

    setCheckbox(name, checked) {
        const field = document.querySelector(`[name="${name}"]`);
        if (field && field.type === 'checkbox') {
            field.checked = checked;
        }
    }

    async saveConfig(e) {
        e.preventDefault();

        const formData = new FormData(e.target);
        const config = this.buildConfigFromForm(formData);

        try {
            const response = await fetch('/api/config', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(config)
            });

            if (!response.ok) throw new Error('Failed to save config');

            this.showToast('Config saved successfully', 'success');
        } catch (error) {
            console.error('Error saving config:', error);
            this.showToast('Failed to save config', 'error');
        }
    }

    buildConfigFromForm(formData) {
        const config = {
            default_provider: formData.get('default_provider') || 'gemini',
            default_model: formData.get('default_model') || null,
            models: {
                providers: {}
            },
            memory: {
                backend: formData.get('memory.backend') || 'sqlite',
                embedding_model: formData.get('memory.embedding_model') || ''
            },
            channels: {
                telegram: [{}]
            },
            http_request: {
                enabled: formData.get('http_request.enabled') === 'on',
                search_provider: formData.get('http_request.search_provider') || 'duckduckgo',
                allowed_domains: []
            },
            browser: {
                enabled: formData.get('browser.enabled') === 'on'
            },
            composio: {
                enabled: formData.get('composio.enabled') === 'on',
                api_key: formData.get('composio.api_key') || '',
                entity_id: formData.get('composio.entity_id') || 'default'
            }
        };

        // Build providers
        const providerKeys = {
            'providers.gemini.api_key': 'gemini',
            'providers.openai.api_key': 'openai',
            'providers.anthropic.api_key': 'anthropic',
            'providers.openrouter.api_key': 'openrouter',
            'providers.kilo.api_key': 'kilo'
        };

        for (const [key, providerName] of Object.entries(providerKeys)) {
            const value = formData.get(key);
            if (value) {
                config.models.providers[providerName] = {
                    api_key: value,
                    base_url: null
                };
            }
        }

        // Build Telegram config
        const telegramToken = formData.get('channels.telegram[0].bot_token');
        const allowFrom = formData.get('channels.telegram[0].allow_from');

        if (telegramToken) {
            config.channels.telegram[0] = {
                account_id: 'main',
                bot_token: telegramToken,
                allow_from: allowFrom ? allowFrom.split(',').map(s => s.trim()) : [],
                group_policy: 'allowlist'
            };
        }

        return config;
    }

    // Chat Functionality
    setupChat() {
        const chatInput = document.getElementById('chat-input');
        const sendBtn = document.getElementById('send-btn');

        chatInput.addEventListener('keypress', (e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                this.sendMessage();
            }
        });

        sendBtn.addEventListener('click', () => this.sendMessage());

        // Connect to WebSocket
        this.connectWebSocket();
    }

    connectWebSocket() {
        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const wsUrl = `${protocol}//${window.location.host}/ws/chat`;

        try {
            this.ws = new WebSocket(wsUrl);

            this.ws.onopen = () => {
                this.isConnected = true;
                this.updateConnectionStatus(true);
                this.addSystemMessage('Connected to OpenPaw agent');
                this.enableChatInput();
            };

            this.ws.onclose = () => {
                this.isConnected = false;
                this.updateConnectionStatus(false);
                this.addSystemMessage('Disconnected from agent. Reconnecting...');
                this.disableChatInput();

                // Attempt reconnection
                setTimeout(() => this.connectWebSocket(), 3000);
            };

            this.ws.onerror = (error) => {
                console.error('WebSocket error:', error);
                this.addSystemMessage('Connection error');
            };

            this.ws.onmessage = (event) => {
                try {
                    const data = JSON.parse(event.data);
                    this.handleIncomingMessage(data);
                } catch (error) {
                    console.error('Error parsing message:', error);
                }
            };
        } catch (error) {
            console.error('Failed to connect WebSocket:', error);
            this.addSystemMessage('Failed to connect to chat server');
        }
    }

    handleIncomingMessage(data) {
        if (data.type === 'response' || data.type === 'message') {
            this.addMessage(data.content || data.text, 'assistant');
        } else if (data.type === 'error') {
            this.addSystemMessage(`Error: ${data.message}`);
        } else if (data.type === 'status') {
            this.addSystemMessage(data.status);
        }
    }

    sendMessage() {
        const chatInput = document.getElementById('chat-input');
        const message = chatInput.value.trim();

        if (!message || !this.isConnected) return;

        // Add user message to chat
        this.addMessage(message, 'user');

        // Send via WebSocket
        this.ws.send(JSON.stringify({
            type: 'message',
            content: message
        }));

        // Clear input
        chatInput.value = '';
    }

    addMessage(content, type) {
        const messagesContainer = document.getElementById('chat-messages');
        const messageDiv = document.createElement('div');
        messageDiv.className = `message ${type}-message`;

        const contentDiv = document.createElement('div');
        contentDiv.className = 'message-content';
        contentDiv.textContent = content;

        messageDiv.appendChild(contentDiv);
        messagesContainer.appendChild(messageDiv);

        // Scroll to bottom
        messagesContainer.scrollTop = messagesContainer.scrollHeight;
    }

    addSystemMessage(text) {
        const messagesContainer = document.getElementById('chat-messages');
        const messageDiv = document.createElement('div');
        messageDiv.className = 'message system-message';

        const contentDiv = document.createElement('div');
        contentDiv.className = 'message-content';
        contentDiv.textContent = text;

        messageDiv.appendChild(contentDiv);
        messagesContainer.appendChild(messageDiv);

        messagesContainer.scrollTop = messagesContainer.scrollHeight;
    }

    updateConnectionStatus(connected) {
        const statusEl = document.getElementById('connection-status');
        const statusText = statusEl.querySelector('.status-text');

        if (connected) {
            statusEl.classList.add('connected');
            statusText.textContent = 'Connected';
        } else {
            statusEl.classList.remove('connected');
            statusText.textContent = 'Disconnected';
        }
    }

    enableChatInput() {
        const chatInput = document.getElementById('chat-input');
        const sendBtn = document.getElementById('send-btn');
        chatInput.disabled = false;
        sendBtn.disabled = false;
        chatInput.placeholder = 'Type your message...';
    }

    disableChatInput() {
        const chatInput = document.getElementById('chat-input');
        const sendBtn = document.getElementById('send-btn');
        chatInput.disabled = true;
        sendBtn.disabled = true;
        chatInput.placeholder = 'Connecting...';
    }

    // Toast Notifications
    showToast(message, type = 'info') {
        const toast = document.createElement('div');
        toast.className = `toast ${type}`;
        toast.textContent = message;
        document.body.appendChild(toast);

        setTimeout(() => {
            toast.remove();
        }, 3000);
    }
}

// Initialize the app when DOM is ready
document.addEventListener('DOMContentLoaded', () => {
    window.openPawUI = new OpenPawUI();
});
