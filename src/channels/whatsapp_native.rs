use anyhow::{Result, anyhow};
use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;

use super::root::{Channel, ParsedMessage};

pub struct WhatsAppNativeChannel {
    bridge_url: String,
    _webhook_url: String,
    allow_from: Vec<String>,
    client: Client,
}

#[derive(Serialize)]
struct BridgeSendRequest {
    to: String,
    content: String,
}

impl WhatsAppNativeChannel {
    pub fn new(bridge_url: String, webhook_url: String, allow_from: Vec<String>) -> Self {
        Self {
            bridge_url: if bridge_url.is_empty() {
                "http://localhost:18790".to_string()
            } else {
                bridge_url
            },
            _webhook_url: webhook_url,
            allow_from,
            client: Client::new(),
        }
    }

    pub fn is_number_allowed(&self, phone: &str) -> bool {
        if self.allow_from.is_empty() {
            return true;
        }
        for allowed in &self.allow_from {
            if allowed == "*" || allowed == phone {
                return true;
            }
        }
        false
    }
}

#[async_trait]
impl Channel for WhatsAppNativeChannel {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "whatsapp_native"
    }

    fn account_id(&self) -> &str {
        "default"
    }

    fn send_message(&self, chat_id: &str, text: &str) -> Result<()> {
        let url = format!("{}/send", self.bridge_url);
        let req = BridgeSendRequest {
            to: chat_id.to_string(),
            content: text.to_string(),
        };

        // Bridge uses async, but Channel trait is sync (historical).
        // Ideally we'd update Channel trait, but for now we block.
        let client = self.client.clone();
        tokio::task::block_in_place(move || {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let resp = client.post(&url).json(&req).send().await?;
                if !resp.status().is_success() {
                    return Err(anyhow!("Bridge error: {}", resp.status()));
                }
                Ok(())
            })
        })
    }

    fn poll_updates(&self) -> Result<Vec<ParsedMessage>> {
        // Webhook-based, bridge calls OpenPaw
        Ok(Vec::new())
    }

    fn health_check(&self) -> bool {
        // Check if bridge is up
        let url = format!("{}/send", self.bridge_url);
        let client = self.client.clone();
        tokio::task::block_in_place(move || {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async { client.get(&url).send().await.is_ok() })
        })
    }
}
