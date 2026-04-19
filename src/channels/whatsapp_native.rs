use anyhow::{Result, anyhow};
use async_trait::async_trait;
use rand::Rng;
use reqwest::Client;
use serde::Serialize;
use std::time::Duration;

use super::root::{Channel, ParsedMessage};

pub struct WhatsAppNativeChannel {
    bridge_url: String,
    webhook_url: String,
    allow_from: Vec<String>,
    client: Client,
}

#[derive(Serialize)]
struct BridgeSendRequest {
    to: String,
    content: String,
}

#[derive(Serialize)]
struct BridgeTypingRequest {
    chat_id: String,
    is_typing: bool,
}

impl WhatsAppNativeChannel {
    pub fn new(bridge_url: String, webhook_url: String, allow_from: Vec<String>) -> Self {
        Self {
            bridge_url: if bridge_url.is_empty() {
                "http://localhost:18790".to_string()
            } else {
                bridge_url
            },
            webhook_url,
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

    async fn set_typing(&self, chat_id: &str, is_typing: bool) -> Result<()> {
        let url = format!("{}/typing", self.bridge_url);
        let req = BridgeTypingRequest {
            chat_id: chat_id.to_string(),
            is_typing,
        };
        let _ = self.client.post(&url).json(&req).send().await;
        Ok(())
    }

    async fn update_presence(&self) -> Result<()> {
        let url = format!("{}/presence", self.bridge_url);
        let _ = self.client.post(&url).send().await;
        Ok(())
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

        let client = self.client.clone();
        let bridge_url = self.bridge_url.clone();

        // Safety: Anti-ban measures
        // 1. Random delay before starting "typing"
        // 2. Typing indicator for a duration based on message length
        // 3. Random delay before final send

        tokio::task::block_in_place(move || {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut rng = rand::thread_rng();

                // Initial pause (1-3 seconds)
                tokio::time::sleep(Duration::from_millis(rng.gen_range(1000..3000))).await;

                // Set typing = true
                let typing_req = BridgeTypingRequest {
                    chat_id: chat_id.to_string(),
                    is_typing: true,
                };
                let _ = client
                    .post(format!("{}/typing", bridge_url))
                    .json(&typing_req)
                    .send()
                    .await;

                // Typing duration based on text length (approx 150 chars per minute = 2.5 chars per sec)
                let typing_ms = (text.len() as u64 * 400).min(10000).max(2000);
                tokio::time::sleep(Duration::from_millis(typing_ms)).await;

                // Set typing = false
                let typing_req_off = BridgeTypingRequest {
                    chat_id: chat_id.to_string(),
                    is_typing: false,
                };
                let _ = client
                    .post(format!("{}/typing", bridge_url))
                    .json(&typing_req_off)
                    .send()
                    .await;

                // Final small delay
                tokio::time::sleep(Duration::from_millis(rng.gen_range(500..1500))).await;

                let resp = client.post(&url).json(&req).send().await?;
                if !resp.status().is_success() {
                    return Err(anyhow!("Bridge error: {}", resp.status()));
                }
                Ok(())
            })
        })
    }

    fn poll_updates(&self) -> Result<Vec<ParsedMessage>> {
        Ok(Vec::new())
    }

    fn health_check(&self) -> bool {
        let url = format!("{}/presence", self.bridge_url);
        let client = self.client.clone();
        tokio::task::block_in_place(move || {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async { client.post(&url).send().await.is_ok() })
        })
    }

    fn send_typing(&self, chat_id: &str) {
        let client = self.client.clone();
        let url = format!("{}/typing", self.bridge_url);
        let chat_id = chat_id.to_string();
        tokio::spawn(async move {
            let req = BridgeTypingRequest {
                chat_id,
                is_typing: true,
            };
            let _ = client.post(&url).json(&req).send().await;
        });
    }
}
