use super::{Tool, ToolContext, ToolResult};
use crate::config::Config;
use anyhow::Result;
use async_trait::async_trait;
use lettre::AsyncTransport;
use serde_json::Value;
use std::sync::Arc;

pub struct EmailSendTool {
    pub config: Arc<Config>,
}

pub struct EmailReadTool {
    pub config: Arc<Config>,
}

pub struct EmailCheckTool {
    pub config: Arc<Config>,
}

fn fail_disabled() -> ToolResult {
    ToolResult::fail("Email is not enabled. Set `email.enabled = true` and configure address + password in config.json.")
}

fn fail_no_address() -> ToolResult {
    ToolResult::fail("Email address not configured in config.json.")
}

fn fail_no_password() -> ToolResult {
    ToolResult::fail("Email password not configured in config.json.")
}

#[async_trait]
impl Tool for EmailSendTool {
    fn name(&self) -> &str {
        "email_send"
    }

    fn description(&self) -> &str {
        "Send an email to one or more recipients. \
         Requires email.enabled, email.address, and email.password in config.json. \
         Supports plain text and HTML bodies, CC, BCC, and Reply-To."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "to": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of recipient email addresses"
                },
                "subject": {
                    "type": "string",
                    "description": "Email subject line"
                },
                "body": {
                    "type": "string",
                    "description": "Email body (plain text)"
                },
                "html": {
                    "type": "string",
                    "description": "Optional HTML body (if provided, sent as multipart alternative)"
                },
                "cc": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional CC recipients"
                },
                "bcc": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional BCC recipients"
                },
                "reply_to": {
                    "type": "string",
                    "description": "Optional Reply-To address"
                }
            },
            "required": ["to", "subject", "body"]
        }"#
            .to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let email_cfg = &self.config.email;
        if !email_cfg.enabled {
            return Ok(fail_disabled());
        }
        let from_addr = match &email_cfg.address {
            Some(a) if !a.is_empty() => a.clone(),
            _ => return Ok(fail_no_address()),
        };
        let password = match &email_cfg.password {
            Some(p) if !p.is_empty() => p.clone(),
            _ => return Ok(fail_no_password()),
        };
        let smtp_host = email_cfg.resolve_smtp_host();
        let smtp_port = email_cfg.smtp_port;

        let to_list: Vec<String> = match args.get("to") {
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            _ => {
                return Ok(ToolResult::fail(
                    "Missing or invalid 'to' field (must be an array of email addresses).",
                ))
            }
        };
        if to_list.is_empty() {
            return Ok(ToolResult::fail(
                "'to' must contain at least one email address.",
            ));
        }

        let subject = match args.get("subject").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => return Ok(ToolResult::fail("Missing required 'subject' parameter.")),
        };
        let body = match args.get("body").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            _ => return Ok(ToolResult::fail("Missing required 'body' parameter.")),
        };
        let html_body = args
            .get("html")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let cc: Vec<String> = args
            .get("cc")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let bcc: Vec<String> = args
            .get("bcc")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let reply_to = args
            .get("reply_to")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mut email_builder = lettre::message::Message::builder();
        email_builder = email_builder.from(
            lettre::message::Mailbox::new(
                Some("OpenPaw".to_string()),
                from_addr
                    .parse()
                    .map_err(|e| anyhow::anyhow!("Invalid from address: {}", e))?,
            ),
        );

        for addr in &to_list {
            let mailbox: lettre::message::Mailbox = addr
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid to address '{}': {}", addr, e))?;
            email_builder = email_builder.to(mailbox);
        }

        for addr in &cc {
            let mailbox: lettre::message::Mailbox = addr
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid CC address '{}': {}", addr, e))?;
            email_builder = email_builder.cc(mailbox);
        }

        for addr in &bcc {
            let mailbox: lettre::message::Mailbox = addr
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid BCC address '{}': {}", addr, e))?;
            email_builder = email_builder.bcc(mailbox);
        }

        if let Some(rt) = &reply_to {
            let mailbox: lettre::message::Mailbox = rt
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid Reply-To address '{}': {}", rt, e))?;
            email_builder = email_builder.reply_to(mailbox);
        }

        email_builder = email_builder.subject(&subject);

        let message = if let Some(ref html) = html_body {
            let alternative = lettre::message::MultiPart::alternative()
                .singlepart(
                    lettre::message::SinglePart::builder()
                        .header(lettre::message::header::ContentType::TEXT_PLAIN)
                        .body(body),
                )
                .singlepart(
                    lettre::message::SinglePart::builder()
                        .header(lettre::message::header::ContentType::TEXT_HTML)
                        .body(html.clone()),
                );
            email_builder
                .multipart(alternative)
                .map_err(|e| anyhow::anyhow!("Failed to build email: {}", e))?
        } else {
            email_builder
                .body(body)
                .map_err(|e| anyhow::anyhow!("Failed to build email: {}", e))?
        };

        let creds =
            lettre::transport::smtp::authentication::Credentials::new(from_addr, password);

        let use_starttls = smtp_port == 587;
        let use_tls = smtp_port == 465;

        let mailer = if use_starttls {
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::starttls_relay(&smtp_host)
                .map_err(|e| anyhow::anyhow!("Failed to create STARTTLS transport: {}", e))?
                .port(smtp_port)
                .credentials(creds)
                .build()
        } else if use_tls {
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::relay(&smtp_host)
                .map_err(|e| anyhow::anyhow!("Failed to create SMTPS transport: {}", e))?
                .port(smtp_port)
                .credentials(creds)
                .build()
        } else {
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous(&smtp_host)
                .port(smtp_port)
                .credentials(creds)
                .build()
        };

        match mailer.send(message).await {
            Ok(_) => Ok(ToolResult::ok(format!(
                "Email sent successfully to {} recipient(s)",
                to_list.len()
            ))),
            Err(e) => Ok(ToolResult::fail(format!("Failed to send email: {}", e))),
        }
    }
}

#[async_trait]
impl Tool for EmailReadTool {
    fn name(&self) -> &str {
        "email_read"
    }

    fn description(&self) -> &str {
        "Read emails from the inbox via IMAP. \
         Returns subject, from, date, and body preview for each message. \
         Auto-detects IMAP server from your email domain. \
         Only requires email.address and email.password in config.json."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "folder": {
                    "type": "string",
                    "description": "IMAP folder to read from (default: INBOX)"
                },
                "count": {
                    "type": "integer",
                    "description": "Number of recent emails to fetch (default: 5, max: 20)"
                },
                "unseen_only": {
                    "type": "boolean",
                    "description": "Only fetch unread emails (default: false)"
                },
                "search_from": {
                    "type": "string",
                    "description": "Filter emails from a specific sender address"
                },
                "search_subject": {
                    "type": "string",
                    "description": "Filter emails by subject substring"
                }
            }
        }"#
            .to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let email_cfg = &self.config.email;
        if !email_cfg.enabled {
            return Ok(fail_disabled());
        }
        let address = match &email_cfg.address {
            Some(a) if !a.is_empty() => a.clone(),
            _ => return Ok(fail_no_address()),
        };
        let password = match &email_cfg.password {
            Some(p) if !p.is_empty() => p.clone(),
            _ => return Ok(fail_no_password()),
        };

        let folder = args
            .get("folder")
            .and_then(|v| v.as_str())
            .unwrap_or("INBOX")
            .to_string();
        let count = args
            .get("count")
            .and_then(|v| v.as_i64())
            .unwrap_or(5)
            .min(20) as usize;
        let unseen_only = args
            .get("unseen_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let search_from = args
            .get("search_from")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let search_subject = args
            .get("search_subject")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let imap_host = email_cfg.resolve_imap_host();
        let imap_port = email_cfg.imap_port;

        let result = tokio::task::spawn_blocking(move || {
            read_emails_blocking(
                &imap_host,
                imap_port,
                &address,
                &password,
                &folder,
                count,
                unseen_only,
                search_from.as_deref(),
                search_subject.as_deref(),
                false,
            )
        })
        .await
        .map_err(|e| anyhow::anyhow!("IMAP task join error: {}", e))??;

        Ok(result)
    }
}

#[async_trait]
impl Tool for EmailCheckTool {
    fn name(&self) -> &str {
        "email_check"
    }

    fn description(&self) -> &str {
        "Check how many unread emails are in the inbox. \
         Returns just a count of unseen messages. Lightweight — no body fetching. \
         Auto-detects IMAP server from your email domain."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "folder": {
                    "type": "string",
                    "description": "IMAP folder to check (default: INBOX)"
                }
            }
        }"#
            .to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let email_cfg = &self.config.email;
        if !email_cfg.enabled {
            return Ok(fail_disabled());
        }
        let address = match &email_cfg.address {
            Some(a) if !a.is_empty() => a.clone(),
            _ => return Ok(fail_no_address()),
        };
        let password = match &email_cfg.password {
            Some(p) if !p.is_empty() => p.clone(),
            _ => return Ok(fail_no_password()),
        };

        let folder = args
            .get("folder")
            .and_then(|v| v.as_str())
            .unwrap_or("INBOX")
            .to_string();

        let imap_host = email_cfg.resolve_imap_host();
        let imap_port = email_cfg.imap_port;

        let result = tokio::task::spawn_blocking(move || {
            read_emails_blocking(
                &imap_host,
                imap_port,
                &address,
                &password,
                &folder,
                0,
                true,
                None,
                None,
                true,
            )
        })
        .await
        .map_err(|e| anyhow::anyhow!("IMAP task join error: {}", e))??;

        Ok(result)
    }
}

fn read_emails_blocking(
    imap_host: &str,
    imap_port: u16,
    address: &str,
    password: &str,
    folder: &str,
    count: usize,
    unseen_only: bool,
    search_from: Option<&str>,
    search_subject: Option<&str>,
    check_only: bool,
) -> Result<ToolResult> {
    let client = imap::ClientBuilder::new(imap_host, imap_port)
        .connect()
        .map_err(|e| anyhow::anyhow!("IMAP connection failed: {}", e))?;

    let mut session = match client.login(address, password) {
        Ok(s) => s,
        Err((e, _)) => {
            return Ok(ToolResult::fail(format!(
                "IMAP login failed: {}",
                e
            )));
        }
    };

    session
        .select(folder)
        .map_err(|e| anyhow::anyhow!("Failed to select folder '{}': {}", folder, e))?;

    if check_only {
        let unseen = session
            .uid_search("UNSEEN")
            .map_err(|e| anyhow::anyhow!("IMAP SEARCH failed: {}", e))?;
        let total = session
            .uid_search("ALL")
            .map_err(|e| anyhow::anyhow!("IMAP SEARCH failed: {}", e))?;
        session
            .logout()
            .map_err(|e| anyhow::anyhow!("IMAP logout error: {}", e))?;
        return Ok(ToolResult::ok(format!(
            "Inbox: {} total, {} unread",
            total.len(),
            unseen.len()
        )));
    }

    let mut search_criteria = String::new();
    if unseen_only {
        search_criteria.push_str("UNSEEN ");
    }
    if let Some(from) = search_from {
        search_criteria
            .push_str(&format!("FROM \"{}\" ", from.replace('"', "\\\"")));
    }
    if let Some(subj) = search_subject {
        search_criteria
            .push_str(&format!("SUBJECT \"{}\" ", subj.replace('"', "\\\"")));
    }
    if search_criteria.is_empty() {
        search_criteria = "ALL".to_string();
    }

    let uids = session
        .uid_search(&search_criteria)
        .map_err(|e| anyhow::anyhow!("IMAP SEARCH failed: {}", e))?;

    let mut uid_vec: Vec<u32> = uids.into_iter().collect();
    uid_vec.sort_by(|a, b| b.cmp(a));

    let fetch_count = count.min(uid_vec.len());
    if fetch_count == 0 {
        session
            .logout()
            .map_err(|e| anyhow::anyhow!("IMAP logout error: {}", e))?;
        return Ok(ToolResult::ok("No emails found matching criteria."));
    }

    let fetch_uids: Vec<u32> = uid_vec[..fetch_count].to_vec();
    let uid_set: String = fetch_uids
        .iter()
        .map(|u| u.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let messages = session
        .uid_fetch(&uid_set, "(ENVELOPE BODY.PEEK[]<0.2000>)")
        .map_err(|e| anyhow::anyhow!("IMAP FETCH failed: {}", e))?;

    let mut results = Vec::new();
    for msg in messages.iter() {
        let envelope = match msg.envelope() {
            Some(e) => e,
            None => {
                results.push("(no envelope data)\n".to_string());
                continue;
            }
        };

        let subject = envelope
            .subject
            .as_ref()
            .map(|s| String::from_utf8_lossy(s).to_string())
            .unwrap_or_else(|| "(no subject)".to_string());

        let from_str = envelope
            .from
            .as_ref()
            .and_then(|addrs| addrs.first())
            .map(|a| {
                let name = a
                    .name
                    .as_ref()
                    .map(|n| String::from_utf8_lossy(n).to_string())
                    .unwrap_or_default();
                let mailbox = a
                    .mailbox
                    .as_ref()
                    .map(|m| String::from_utf8_lossy(m).to_string())
                    .unwrap_or_default();
                let host = a
                    .host
                    .as_ref()
                    .map(|h| String::from_utf8_lossy(h).to_string())
                    .unwrap_or_default();
                if name.is_empty() {
                    format!("{}@{}", mailbox, host)
                } else {
                    format!("{} <{}@{}>", name, mailbox, host)
                }
            })
            .unwrap_or_else(|| "(unknown)".to_string());

        let date = envelope
            .date
            .as_ref()
            .map(|d| String::from_utf8_lossy(d).to_string())
            .unwrap_or_else(|| "(no date)".to_string());

        let body_snippet = msg
            .body()
            .map(|b| {
                let text = String::from_utf8_lossy(b);
                text.chars().take(500).collect::<String>()
            })
            .unwrap_or_else(|| "(no body preview)".to_string());

        results.push(format!(
            "---\nFrom: {}\nDate: {}\nSubject: {}\nPreview: {}",
            from_str, date, subject, body_snippet
        ));
    }

    session
        .logout()
        .map_err(|e| anyhow::anyhow!("IMAP logout error: {}", e))?;

    let summary = format!(
        "Fetched {} of {} matching email(s).\n{}",
        fetch_count,
        uid_vec.len(),
        results.join("\n")
    );
    Ok(ToolResult::ok(summary))
}