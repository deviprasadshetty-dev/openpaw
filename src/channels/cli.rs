use crate::channels::root::{Channel, ParsedMessage};
use anyhow::Result;
use crossbeam_channel::{Receiver, unbounded};
use std::any::Any;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use tracing::error;

pub struct CliChannel {
    rx: Arc<Mutex<Receiver<String>>>,
    account_id: String,
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
                // print prompt
                print!("> ");
                let _ = io::stdout().flush();

                match stdin.read_line(&mut buffer) {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let trimmed = buffer.trim().to_string();
                        if !trimmed.is_empty()
                            && tx.send(trimmed).is_err() {
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
        }
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
        println!("{}", text);
        Ok(())
    }

    fn send_stream_chunk(&self, _chat_id: &str, text: &str) -> Result<()> {
        print!("{}", text);
        let _ = io::stdout().flush();
        Ok(())
    }

    fn poll_updates(&self) -> Result<Vec<ParsedMessage>> {
        let mut messages = Vec::new();
        let rx = self.rx.lock().unwrap();

        // Drain all available input
        while let Ok(content) = rx.try_recv() {
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
