use anyhow::{Result, anyhow};
use reqwest::Client;
use serde_json::Value;
use std::fs::File;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{error, warn};

use super::root::{Channel, ParsedMessage};

pub const API_VERSION: &str = "v18.0";

pub struct WhatsAppChannel {
    access_token: String,
    phone_number_id: String,
    verify_token: String,
    allow_from: Vec<String>,
    group_allow_from: Vec<String>,
    group_policy: String,
    client: Client,
}

impl WhatsAppChannel {
    pub fn new(
        access_token: String,
        phone_number_id: String,
        verify_token: String,
        allow_from: Vec<String>,
        group_allow_from: Vec<String>,
        group_policy: String,
    ) -> Self {
        Self {
            access_token,
            phone_number_id,
            verify_token,
            allow_from,
            group_allow_from,
            group_policy,
            client: Client::new(),
        }
    }

    pub fn get_verify_token(&self) -> &str {
        &self.verify_token
    }

    pub fn is_number_allowed(&self, phone: &str) -> bool {
        for allowed in &self.allow_from {
            if allowed == "*" || allowed == phone {
                return true;
            }
        }
        false
    }

    pub fn normalize_phone(phone: &str) -> String {
        if phone.is_empty() {
            return String::new();
        }
        if phone.starts_with('+') {
            return phone.to_string();
        }
        format!("+{}", phone)
    }

    fn now_epoch_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    pub fn parse_webhook_payload(&self, payload: &str) -> Result<Vec<ParsedMessage>> {
        let val: Value = serde_json::from_str(payload)?;
        let mut result = Vec::new();

        let entries = val.get("entry").and_then(|v| v.as_array());
        if let Some(entries) = entries {
            for entry in entries {
                let changes = entry.get("changes").and_then(|v| v.as_array());
                if let Some(changes) = changes {
                    for change in changes {
                        let value = change.get("value");
                        if let Some(value_obj) = value {
                            let messages = value_obj.get("messages").and_then(|v| v.as_array());
                            if let Some(messages) = messages {
                                for msg in messages {
                                    if let Some(from) = msg.get("from").and_then(|v| v.as_str()) {
                                        let normalized = Self::normalize_phone(from);

                                        // Check Group Policy
                                        let is_group_msg = msg
                                            .get("context")
                                            .and_then(|ctx| ctx.get("group_jid"))
                                            .is_some();

                                        if !is_group_msg {
                                            if !self.allow_from.is_empty()
                                                && !self.is_number_allowed(&normalized)
                                            {
                                                continue;
                                            }
                                        } else {
                                            if self.group_policy == "disabled" {
                                                continue;
                                            }
                                            if self.group_policy != "open" {
                                                let effective_allow_from =
                                                    if !self.group_allow_from.is_empty() {
                                                        &self.group_allow_from
                                                    } else {
                                                        &self.allow_from
                                                    };

                                                let allowed = effective_allow_from
                                                    .iter()
                                                    .any(|a| a == "*" || a == &normalized);
                                                if effective_allow_from.is_empty() || !allowed {
                                                    continue;
                                                }
                                            }
                                        }

                                        // Extract Text
                                        let body = msg
                                            .get("text")
                                            .and_then(|t| t.get("body"))
                                            .and_then(|b| b.as_str());

                                        if let Some(body) = body {
                                            if body.is_empty() {
                                                continue;
                                            }

                                            let _timestamp = msg
                                                .get("timestamp")
                                                .and_then(|ts| ts.as_str())
                                                .and_then(|ts| ts.parse::<u64>().ok())
                                                .unwrap_or_else(Self::now_epoch_secs);

                                            let mut parsed_msg = ParsedMessage::new(
                                                &normalized,
                                                &normalized, // chat_id = sender for individuals
                                                body,
                                                &normalized,
                                            );
                                            parsed_msg.is_group = is_group_msg;

                                            result.push(parsed_msg);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    pub async fn download_media_from_payload(&self, payload: &str) -> Option<String> {
        if self.access_token.is_empty() {
            return None;
        }

        // Quick parse to find image ID
        let val: Value = serde_json::from_str(payload).ok()?;
        // Simplify navigation for brevity, assuming standard structure similar to parse logic
        let msg = val
            .get("entry")?
            .get(0)?
            .get("changes")?
            .get(0)?
            .get("value")?
            .get("messages")?
            .get(0)?;
        let media_id = msg.get("image")?.get("id")?.as_str()?;

        // Step 1: Get media URL
        let info_url = format!("https://graph.facebook.com/{}/{}", API_VERSION, media_id);
        let auth_header = format!("Bearer {}", self.access_token);

        let info_resp = self
            .client
            .get(&info_url)
            .header("Authorization", &auth_header)
            .send()
            .await
            .ok()?;

        let info_json: Value = info_resp.json().await.ok()?;
        let media_url = info_json.get("url")?.as_str()?;

        // Step 2: Download bytes
        let media_bytes = self
            .client
            .get(media_url)
            .header("Authorization", &auth_header)
            .send()
            .await
            .ok()?
            .bytes()
            .await
            .ok()?;

        // Step 3: Write to tmp file
        let rand_id = rand::random::<u64>();
        let local_path = format!("/tmp/whatsapp_{:x}.dat", rand_id);

        let mut file = File::create(&local_path).ok()?;
        file.write_all(&media_bytes).ok()?;

        Some(format!("[IMAGE:{}]", local_path))
    }

    pub async fn send_message(&self, recipient: &str, text: &str) -> Result<()> {
        let url = format!(
            "https://graph.facebook.com/{}/{}/messages",
            API_VERSION, self.phone_number_id
        );

        let to = if recipient.starts_with('+') {
            &recipient[1..]
        } else {
            recipient
        };

        // Construct body using serde_json json! macro for safety
        let body = serde_json::json!({
            "messaging_product": "whatsapp",
            "recipient_type": "individual",
            "to": to,
            "type": "text",
            "text": {
                "preview_url": false,
                "body": text
            }
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            error!("WhatsApp API error {}: {}", status, err_text);
            return Err(anyhow!("WhatsApp API error"));
        }

        Ok(())
    }
}

impl Channel for WhatsAppChannel {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "whatsapp"
    }

    fn account_id(&self) -> &str {
        "default"
    }

    fn send_message(&self, _chat_id: &str, _text: &str) -> Result<()> {
        // WhatsApp uses async, so we need to block on it
        // In a real implementation, you'd use a runtime or channel
        warn!("WhatsApp send_message called synchronously - needs async runtime");
        Ok(())
    }

    fn poll_updates(&self) -> Result<Vec<ParsedMessage>> {
        // WhatsApp is webhook-based, not polling-based
        Ok(Vec::new())
    }

    fn health_check(&self) -> bool {
        true
    }
}
