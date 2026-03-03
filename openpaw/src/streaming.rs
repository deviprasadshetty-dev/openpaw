use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundStage {
    Chunk,
    Final,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Event {
    pub stage: OutboundStage,
    #[serde(default)]
    pub text: String,
}

pub trait Sink: Send + Sync {
    fn emit(&self, event: Event);

    fn emit_chunk(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.emit(Event {
            stage: OutboundStage::Chunk,
            text: text.to_string(),
        });
    }

    fn emit_final(&self) {
        self.emit(Event {
            stage: OutboundStage::Final,
            text: String::new(),
        });
    }
}
