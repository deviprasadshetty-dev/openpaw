use crate::channels::root::{Channel, ParsedMessage};
use anyhow::Result;
use crossbeam_channel::{Receiver, unbounded};
use crossterm::{
    cursor, execute,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use std::any::Any;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use tracing::error;

use std::sync::atomic::{AtomicBool, Ordering};

pub struct CliChannel {
    rx: Arc<Mutex<Receiver<String>>>,
    account_id: String,
    first_chunk: AtomicBool,
}

impl CliChannel {
    pub fn new(account_id: String) -> Self {
        let (tx, rx) = unbounded();

        // Spawn a thread to read stdin
        thread::spawn(move || {
            let stdin = io::stdin();
            let mut buffer = String::new();

            loop {
                buffer.clear();

                // Print a modern prompt
                let mut stdout = io::stdout();
                let _ = execute!(
                    stdout,
                    SetForegroundColor(Color::Cyan),
                    SetAttribute(Attribute::Bold),
                    Print("\n  ❯ "),
                    SetAttribute(Attribute::Reset),
                    ResetColor,
                );
                let _ = stdout.flush();

                match stdin.read_line(&mut buffer) {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let trimmed = buffer.trim().to_string();
                        if !trimmed.is_empty() && tx.send(trimmed).is_err() {
                            break; // Channel closed
                        }
                    }
                    Err(e) => {
                        error!("Error reading stdin: {}", e);
                        break;
                    }
                }
            }
        });

        Self {
            rx: Arc::new(Mutex::new(rx)),
            account_id,
            first_chunk: AtomicBool::new(true),
        }
    }

    fn render_markdown(&self, text: &str) -> String {
        // Simple ANSI-based markdown-lite renderer for common patterns
        let mut output = text.to_string();

        // Headers
        let re_header = regex::Regex::new(r"(?m)^# (.*)$").unwrap();
        output = re_header
            .replace_all(&output, "\x1b[1;34m$1\x1b[0m")
            .to_string();
        let re_header2 = regex::Regex::new(r"(?m)^## (.*)$").unwrap();
        output = re_header2
            .replace_all(&output, "\x1b[1;36m$1\x1b[0m")
            .to_string();

        // Bold
        let re_bold = regex::Regex::new(r"\*\*(.*?)\*\*").unwrap();
        output = re_bold.replace_all(&output, "\x1b[1m$1\x1b[0m").to_string();

        // Italics
        let re_italic = regex::Regex::new(r"\*(.*?)\*").unwrap();
        output = re_italic
            .replace_all(&output, "\x1b[3m$1\x1b[0m")
            .to_string();

        // Lists
        let re_list = regex::Regex::new(r"(?m)^- (.*)$").unwrap();
        output = re_list
            .replace_all(&output, "  \x1b[32m•\x1b[0m $1")
            .to_string();

        // Code blocks
        let re_code = regex::Regex::new(r"```(.*?)\n([\s\S]*?)```").unwrap();
        output = re_code.replace_all(&output, |caps: &regex::Captures| {
            let lang = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let code = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            format!("\n  \x1b[48;5;235m\x1b[38;5;250m {} \x1b[0m\n  \x1b[48;5;235m\x1b[38;5;255m{}\x1b[0m\n", lang, code.replace("\n", "\n  "))
        }).to_string();

        output
    }
}

impl Channel for CliChannel {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "cli"
    }

    fn account_id(&self) -> &str {
        &self.account_id
    }

    fn send_message(&self, _chat_id: &str, text: &str) -> Result<()> {
        let mut stdout = io::stdout();

        let _ = execute!(
            stdout,
            SetForegroundColor(Color::Green),
            SetAttribute(Attribute::Bold),
            Print("\n╭─ OpenPaw\n"),
            SetAttribute(Attribute::Reset),
            SetForegroundColor(Color::DarkGrey),
            Print("╰─ "),
            ResetColor,
        );

        let rendered = self.render_markdown(text);
        println!("{}", rendered);
        Ok(())
    }

    fn send_stream_chunk(&self, _chat_id: &str, text: &str) -> Result<()> {
        let mut stdout = io::stdout();

        // Handle first chunk to print header
        if self.first_chunk.load(Ordering::Relaxed) {
            // Clear the "Thinking..." line
            let _ = execute!(
                stdout,
                cursor::MoveToColumn(0),
                terminal::Clear(terminal::ClearType::CurrentLine),
                cursor::MoveUp(1),
                terminal::Clear(terminal::ClearType::CurrentLine),
            );

            let _ = execute!(
                stdout,
                SetForegroundColor(Color::Green),
                SetAttribute(Attribute::Bold),
                Print("\n╭─ OpenPaw\n"),
                SetAttribute(Attribute::Reset),
                SetForegroundColor(Color::DarkGrey),
                Print("╰─ "),
                ResetColor,
            );
            self.first_chunk.store(false, Ordering::Relaxed);
        }

        print!("{}", text);
        let _ = stdout.flush();
        Ok(())
    }

    fn send_typing(&self, _chat_id: &str) {
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            cursor::SavePosition,
            SetForegroundColor(Color::Yellow),
            Print("\n  ⠋ Thinking..."),
            ResetColor,
            cursor::RestorePosition,
        );
        let _ = stdout.flush();
    }

    fn poll_updates(&self) -> Result<Vec<ParsedMessage>> {
        let mut messages = Vec::new();
        let rx = self.rx.lock().unwrap();

        // Drain all available input
        while let Ok(content) = rx.try_recv() {
            // Reset streaming header state for the next response
            self.first_chunk.store(true, Ordering::Relaxed);

            messages.push(ParsedMessage::new(
                "user", // sender_id
                "cli",  // chat_id
                &content,
                &self.account_id, // session_key
            ));
        }

        Ok(messages)
    }

    fn health_check(&self) -> bool {
        true
    }
}
