use crate::channels::root::{Channel, ParsedMessage};
use anyhow::{anyhow, Result};
use imap::types::Fetch;
use lettre::{transport::smtp::authentication::Credentials, Message, SmtpTransport, Transport};
use native_tls::TlsConnector;
use std::any::Any;

pub struct EmailChannel {
    pub smtp_user: String,
    pub smtp_pass: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub imap_host: String,
    pub imap_port: u16,
}

impl EmailChannel {
    pub fn new(
        smtp_user: String,
        smtp_pass: String,
        smtp_host: String,
        smtp_port: u16,
        imap_host: String,
        imap_port: u16,
    ) -> Self {
        Self {
            smtp_user,
            smtp_pass,
            smtp_host,
            smtp_port,
            imap_host,
            imap_port,
        }
    }

    fn extract_subject_and_body<'a>(&self, text: &'a str) -> (&'a str, &'a str) {
        if text.starts_with("Subject: ") {
            if let Some(newline_pos) = text.find('\n') {
                let subject = text[9..newline_pos].trim();
                let body = text[newline_pos + 1..].trim_start();
                return (subject, body);
            }
        }
        ("nullclaw Message", text)
    }
}

impl Channel for EmailChannel {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "email"
    }

    fn account_id(&self) -> &str {
        &self.smtp_user
    }

    fn send_message(&self, chat_id: &str, text: &str) -> Result<()> {
        let (subject, body) = self.extract_subject_and_body(text);

        let email = Message::builder()
            .from(self.smtp_user.parse()?)
            .to(chat_id.parse()?)
            .subject(subject)
            .body(String::from(body))?;

        let creds = Credentials::new(self.smtp_user.clone(), self.smtp_pass.clone());

        let mailer = SmtpTransport::relay(&self.smtp_host)?
            .port(self.smtp_port)
            .credentials(creds)
            .build();

        mailer.send(&email)?;
        Ok(())
    }

    fn poll_updates(&self) -> Result<Vec<ParsedMessage>> {
        let tls = TlsConnector::builder().build()?;
        let client = imap::connect(
            (self.imap_host.as_str(), self.imap_port),
            self.imap_host.as_str(),
            &tls,
        )?;
        let mut imap_session = client
            .login(self.smtp_user.clone(), self.smtp_pass.clone())
            .map_err(|e| anyhow!("IMAP login failed: {:?}", e))?;

        imap_session.select("INBOX")?;

        // Search for UNSEEN messages
        let messages = match imap_session.search("UNSEEN") {
            Ok(m) => m,
            Err(e) => return Err(anyhow!("IMAP search failed: {:?}", e)),
        };
        let mut parsed_messages = Vec::new();

        for uid in messages.iter() {
            let fetches = imap_session.fetch(uid.to_string(), "RFC822.HEADER RFC822.TEXT")?;
            for fetch in fetches.iter() {
                let sender = extract_header(fetch, b"From:");
                let subject = extract_header(fetch, b"Subject:");
                let body = extract_body(fetch);

                let mut sender_id = sender.unwrap_or_else(|| "unknown_sender".to_string());

                // Cleanup sender_id to just extract the email if it's formatted like "Name <email>"
                if let Some(start) = sender_id.find('<') {
                    if let Some(end) = sender_id.find('>') {
                        if end > start {
                            sender_id = sender_id[start + 1..end].to_string();
                        }
                    }
                }

                let text_content = format!(
                    "Subject: {}\n\n{}",
                    subject.unwrap_or_default(),
                    body.unwrap_or_default()
                );

                // Mark as seen
                let _ = imap_session.store(uid.to_string(), "+FLAGS (\\Seen)");

                parsed_messages.push(ParsedMessage::new(
                    &sender_id, // sender_id
                    &sender_id, // chat_id (reply back to sender)
                    &text_content,
                    &self.smtp_user, // session_key
                ));
            }
        }

        imap_session.logout()?;
        Ok(parsed_messages)
    }

    fn health_check(&self) -> bool {
        // Simple IMAP connect test
        if let Ok(tls) = TlsConnector::builder().build() {
            if imap::connect(
                (self.imap_host.as_str(), self.imap_port),
                self.imap_host.as_str(),
                &tls,
            )
            .is_ok()
            {
                return true;
            }
        }
        false
    }
}

fn extract_header(fetch: &Fetch, header_name: &[u8]) -> Option<String> {
    if let Some(header) = fetch.header() {
        let lines: Vec<&[u8]> = header.split(|&b| b == b'\n').collect();
        for line in lines {
            if line
                .to_ascii_lowercase()
                .starts_with(&header_name.to_ascii_lowercase())
            {
                let val = &line[header_name.len()..];
                return String::from_utf8(val.to_vec())
                    .ok()
                    .map(|s| s.trim().to_string());
            }
        }
    }
    None
}

fn extract_body(fetch: &Fetch) -> Option<String> {
    if let Some(body) = fetch.text() {
        String::from_utf8(body.to_vec()).ok()
    } else {
        None
    }
}
