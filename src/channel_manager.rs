use crate::bus::Bus;
use crate::channel_loop::{ChannelRuntime, PollingState};
use crate::channels::dispatch::{ChannelRegistry, SupervisedChannel};
use crate::channels::root::Channel;
use crate::config::Config;
use std::sync::Arc;
use std::thread::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerType {
    Polling,
    GatewayLoop,
    WebhookOnly,
    SendOnly,
    NotImplemented,
}

pub struct Entry {
    pub name: String,
    pub account_id: String,
    pub channel: Box<dyn Channel + Send + Sync>,
    pub listener_type: ListenerType,
    pub supervised: SupervisedChannel,
    pub thread: Option<JoinHandle<()>>,
    pub polling_state: Option<PollingState>,
}

pub struct ChannelManager {
    pub config: Arc<Config>,
    pub registry: Arc<ChannelRegistry>,
    pub runtime: Option<Arc<ChannelRuntime>>,
    pub event_bus: Option<Arc<Bus>>,
    pub entries: Vec<Entry>,
}

impl ChannelManager {
    pub fn new(
        config: Arc<Config>,
        registry: Arc<ChannelRegistry>,
    ) -> Self {
        Self {
            config,
            registry,
            runtime: None,
            event_bus: None,
            entries: Vec::new(),
        }
    }

    pub fn set_runtime(&mut self, rt: Arc<ChannelRuntime>) {
        self.runtime = Some(rt);
    }

    pub fn set_event_bus(&mut self, eb: Arc<Bus>) {
        self.event_bus = Some(eb);
    }

    // fn polling_last_activity(state: &PollingState) -> i64 { ... }
    // fn request_polling_stop(state: &PollingState) { ... }
    
    // fn spawn_polling_thread(&mut self, entry_idx: usize) -> Result<()> {
    //     // ...
    //     Ok(())
    // }

    // fn stop_polling_thread(&mut self, entry_idx: usize) {
    //     // ...
    // }



    // fn listener_type_for_field(field_name: &str) -> ListenerType {
    //     let meta = channel_catalog::find_by_key(field_name).expect("missing channel metadata");
    //     Self::listener_type_from_mode(meta.listener_mode)
    // }

    pub fn stop_all(&mut self) {
        // Stop logic
    }
}
