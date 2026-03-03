use anyhow::Result;

pub trait Channel: Send + Sync {
    fn as_any(&self) -> &dyn std::any::Any;
    
    /// Return the channel name (e.g., "telegram", "whatsapp")
    fn name(&self) -> &str;
    
    /// Return the account ID for multi-account channels
    fn account_id(&self) -> &str;
    
    /// Send a text message to a chat
    fn send_message(&self, chat_id: &str, text: &str) -> Result<()>;
    
    /// Poll for updates (for polling-based channels like Telegram)
    /// Returns a list of parsed messages ready for the bus
    fn poll_updates(&self) -> Result<Vec<ParsedMessage>>;
    
    /// Check if the channel is properly configured and healthy
    fn health_check(&self) -> bool;
}

/// A parsed message from any channel
#[derive(Debug, Clone)]
pub struct ParsedMessage {
    pub sender_id: String,
    pub chat_id: String,
    pub content: String,
    pub session_key: String,
    pub is_group: bool,
    pub message_id: Option<i64>,
    pub username: Option<String>,
    pub first_name: Option<String>,
}

impl ParsedMessage {
    pub fn new(
        sender_id: &str,
        chat_id: &str,
        content: &str,
        session_key: &str,
    ) -> Self {
        Self {
            sender_id: sender_id.to_string(),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            session_key: session_key.to_string(),
            is_group: false,
            message_id: None,
            username: None,
            first_name: None,
        }
    }
}
