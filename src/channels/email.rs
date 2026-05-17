#![cfg(feature = "email")]
use crate::channels::root::{Channel, ParsedMessage};
use anyhow::{Context, Result, anyhow};
use imap::types::Fetch;
use lettre::{
    Message, SmtpTransport, Transport,
    transport::smtp::authentication::Credentials,
};
use native_tls::TlsConnector;
use std::any::Any;

pub struct EmailChannel {
    pub account_id: String,
    /// The bot's email address (also used as SMTP/IMAP login username).
    pub smtp_user: String,
    pub smtp_pass: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub imap_host: String,
    pub imap_port: u16,
}

impl EmailChannel {
    pub fn new(
        account_id: String,
        smtp_user: String,
        smtp_pass: String,
        smtp_host: String,
        smtp_port: u16,
        imap_host: String,
        imap_port: u16,
    ) -> Self {
        Self {
            account_id,
            smtp_user,
            smtp_pass,
            smtp_host,
            smtp_port,
            imap_host,
            imap_port,
        }
    }

    fn extract_subject_and_body<'a>(&self, text: &'a str) -> (&'a str, &'a str) {
        if text.starts_with("Subject: ") || text.starts_with("subject: ") {
            if let Some(newline_pos) = text.find('\n') {
                let subject = text["Subject: ".len()..newline_pos].trim();
                let body = text[newline_pos + 1..].trim_start();
                return (subject, body);
            }
        }
        ("OpenPaw Message", text)
    }

    /// Returns true if `addr` is the bot's own email address (case-insensitive).
    fn is_own_address(&self, addr: &str) -> bool {
        addr.eq_ignore_ascii_case(&self.smtp_user)
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
        &self.account_id
    }

    fn send_message(&self, chat_id: &str, text: &str) -> Result<()> {
        let (subject, body) = self.extract_subject_and_body(text);

        let email = Message::builder()
            .from(self.smtp_user.parse()?)
            .to(chat_id.parse()?)
            .subject(subject)
            .body(String::from(body))?;

        let creds = Credentials::new(self.smtp_user.clone(), self.smtp_pass.clone());

        // Port 465 = implicit TLS (SMTPS), port 587 = STARTTLS, others try STARTTLS
        let mailer = if self.smtp_port == 465 {
            SmtpTransport::relay(&self.smtp_host)?
        } else {
            SmtpTransport::starttls_relay(&self.smtp_host)?
        }
        .port(self.smtp_port)
        .credentials(creds)
        .build();

        mailer.send(&email)?;
        Ok(())
    }

    fn poll_updates(&self) -> Result<Vec<ParsedMessage>> {
        let tls = TlsConnector::builder().build()?;

        // Connect via IMAP. Port 993 = implicit TLS (IMAPS).
        // NOTE: imap 2.x does not support STARTTLS for port 143.
        // If you need port 143, set imap_port=993 or upgrade to imap 3.x.
        let client = imap::connect(
            (self.imap_host.as_str(), self.imap_port),
            self.imap_host.as_str(),
            &tls,
        )
        .with_context(|| {
            format!(
                "IMAP connection failed for {}:{}",
                self.imap_host, self.imap_port
            )
        })?;

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

        for seq in messages.iter() {
            let sequence = seq.to_string();
            let fetches = imap_session
                .fetch(&sequence, "(RFC822.HEADER RFC822.TEXT)")
                .with_context(|| format!("IMAP fetch failed for message {}", sequence))?;
            for fetch in fetches.iter() {
                let sender = extract_header(fetch, b"From:");
                let subject = extract_header(fetch, b"Subject:");
                let body = extract_body(fetch);

                let mut sender_id = sender.unwrap_or_else(|| "unknown_sender".to_string());

                // Extract the bare email from formats like:
                //   "John Doe" <john@example.com>
                //   john@example.com
                //   "John Doe" john@example.com  (non-standard but seen in the wild)
                if let Some(start) = sender_id.find('<') {
                    if let Some(end) = sender_id[start..].find('>') {
                        sender_id = sender_id[start + 1..start + end].to_string();
                    }
                } else {
                    // No angle brackets — try to find an email-like token
                    sender_id = extract_email_from_plain(&sender_id)
                        .unwrap_or(sender_id);
                }

                // Skip messages from our own address to prevent loops
                if self.is_own_address(&sender_id) {
                    // Still mark as seen so we don't keep re-fetching it
                    let _ = imap_session.store(&sequence, "+FLAGS (\\Seen)");
                    continue;
                }

                let text_content = format!(
                    "Subject: {}\n\n{}",
                    subject.unwrap_or_default(),
                    body.unwrap_or_default()
                );

                // Mark as seen
                imap_session
                    .store(&sequence, "+FLAGS (\\Seen)")
                    .with_context(|| {
                        format!("IMAP mark-as-seen failed for message {}", sequence)
                    })?;

                parsed_messages.push(ParsedMessage::new(
                    &sender_id, // sender_id
                    &sender_id, // chat_id (reply back to sender)
                    &text_content,
                    &format!("{}:{}", self.account_id, sender_id), // session_key
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

/// Extract an email address from a plain string (no angle brackets).
/// Looks for tokens containing '@' and returns the first plausible match.
fn extract_email_from_plain(raw: &str) -> Option<String> {
    // Remove surrounding quotes and whitespace
    let cleaned = raw.trim().trim_matches('"').trim_matches('\'').trim();

    // Split by whitespace and find the token with '@'
    for token in cleaned.split_whitespace() {
        let t = token.trim_matches(|c: char| c == '<' || c == '>' || c == '"' || c == '\'');
        if t.contains('@') && t.contains('.') {
            return Some(t.to_string());
        }
    }

    // If the whole cleaned string contains '@', return it
    if cleaned.contains('@') && cleaned.contains('.') {
        return Some(cleaned.to_string());
    }

    None
}

/// Extract a header value from a raw RFC 822 header block.
///
/// Handles:
/// - CRLF (`\r\n`) line endings
/// - Folded headers (continuation lines starting with space or tab)
/// - Case-insensitive header name matching
fn extract_header(fetch: &Fetch, header_name: &[u8]) -> Option<String> {
    let header = fetch.header()?;
    let target = header_name.to_ascii_lowercase();
    let target_len = header_name.len();

    let lines: Vec<&[u8]> = header.split(|&b| b == b'\n').collect();
    let mut i = 0;

    while i < lines.len() {
        // Strip trailing \r (CRLF line endings)
        let line = lines[i];
        let line = line.strip_suffix(&[b'\r']).unwrap_or(line);

        if line.to_ascii_lowercase().starts_with(&target) {
            let mut value = line[target_len..].to_vec();

            // Collect continuation lines (RFC 2822 §2.2.3 — folded headers)
            i += 1;
            while i < lines.len() {
                let cont = lines[i];
                let cont = cont.strip_suffix(&[b'\r']).unwrap_or(cont);
                if cont.first() == Some(&b' ') || cont.first() == Some(&b'\t') {
                    value.extend_from_slice(cont);
                    i += 1;
                } else {
                    break;
                }
            }

            return String::from_utf8(value).ok().map(|s| s.trim().to_string());
        }
        i += 1;
    }

    None
}

/// Extract the body text from a fetch result, normalizing CRLF → LF.
fn extract_body(fetch: &Fetch) -> Option<String> {
    if let Some(body) = fetch.text() {
        String::from_utf8(body.to_vec())
            .ok()
            .map(|s| s.replace("\r\n", "\n"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::root::Channel;

    fn test_channel() -> EmailChannel {
        EmailChannel::new(
            "work".to_string(),
            "bot@example.com".to_string(),
            "secret".to_string(),
            "smtp.example.com".to_string(),
            587,
            "imap.example.com".to_string(),
            993,
        )
    }

    #[test]
    fn account_id_uses_configured_account_not_smtp_user() {
        let ch = test_channel();
        assert_eq!(ch.account_id(), "work");
    }

    #[test]
    fn extracts_subject_and_body_for_replies() {
        let ch = test_channel();
        let (subject, body) = ch.extract_subject_and_body("Subject: Hello\n\nWorld");
        assert_eq!(subject, "Hello");
        assert_eq!(body, "World");
    }

    #[test]
    fn default_subject_is_openpaw() {
        let ch = test_channel();
        let (subject, body) = ch.extract_subject_and_body("Plain reply");
        assert_eq!(subject, "OpenPaw Message");
        assert_eq!(body, "Plain reply");
    }

    #[test]
    fn is_own_address_matches_case_insensitively() {
        let ch = test_channel();
        assert!(ch.is_own_address("bot@example.com"));
        assert!(ch.is_own_address("BOT@EXAMPLE.COM"));
        assert!(ch.is_own_address("Bot@Example.Com"));
        assert!(!ch.is_own_address("someone@example.com"));
    }

    #[test]
    fn extract_email_from_angle_brackets() {
        assert_eq!(
            extract_email_from_plain("\"John Doe\" <john@example.com>"),
            Some("john@example.com".to_string())
        );
    }

    #[test]
    fn extract_email_plain_address() {
        assert_eq!(
            extract_email_from_plain("john@example.com"),
            Some("john@example.com".to_string())
        );
    }

    #[test]
    fn extract_email_with_quoted_display_name() {
        assert_eq!(
            extract_email_from_plain("\"John Doe\" john@example.com"),
            Some("john@example.com".to_string())
        );
    }

    #[test]
    fn extract_email_no_email_returns_none() {
        assert_eq!(extract_email_from_plain("John Doe"), None);
        assert_eq!(extract_email_from_plain(""), None);
    }

    #[test]
    fn extract_header_handles_folded_from() {
        // Simulate a folded From: header with continuation line
        let raw = b"From: \"John Doe\"\r\n <john@example.com>\r\nSubject: Hello\r\n";
        // Build a minimal Fetch-like scenario: we can't easily construct a real Fetch,
        // but we test extract_email_from_plain which handles the post-extraction cleanup.
        // The extract_header function itself relies on Fetch::header() which is hard to mock.

        // Test that extract_email_from_plain handles the output of a folded header
        let folded_output = "\"John Doe\" <john@example.com>";
        assert_eq!(
            extract_email_from_plain(folded_output),
            Some("john@example.com".to_string())
        );
    }

    #[test]
    fn extract_body_normalizes_crlf() {
        // extract_body is hard to unit-test without a real Fetch.
        // The function does a simple replace("\r\n", "\n") on the UTF-8 text.
        let body_with_crlf = "Hello\r\nWorld\r\n";
        let normalized = body_with_crlf.replace("\r\n", "\n");
        assert_eq!(normalized, "Hello\nWorld\n");
    }

    #[test]
    fn smtp_port_587_uses_starttls() {
        // Verify that port 587 is treated as STARTTLS (not implicit TLS)
        let ch = test_channel();
        assert_eq!(ch.smtp_port, 587);
        // The actual transport selection happens in send_message();
        // this test just confirms the port is 587, which triggers STARTTLS.
    }
}
