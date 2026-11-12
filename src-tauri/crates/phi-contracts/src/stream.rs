//! Transient model stream events.
//!
//! Deltas are for live rendering only; they are deliberately **not** written
//! to the event journal. The durable record is the final assistant message.

use std::sync::{Arc, RwLock};

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStreamEvent {
    pub run_id: String,
    pub call_id: String,
    /// Monotonic index within the response (content block / output index).
    pub index: u32,
    /// `text`, `tool_use_start`, `tool_use_delta`, `tool_use_end`, `done`.
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

pub trait StreamPort: Send + Sync {
    fn on_model_event(&self, event: &ModelStreamEvent);
}

/// Registration point for the active shell. A headless node writes JSONL;
/// a desktop shell forwards to the webview. With no sink registered the
/// gateway stays non-streaming.
#[derive(Default)]
pub struct StreamHub {
    sink: RwLock<Option<Arc<dyn StreamPort>>>,
}

impl StreamHub {
    pub fn new(sink: Option<Arc<dyn StreamPort>>) -> Self {
        Self {
            sink: RwLock::new(sink),
        }
    }

    pub fn set_sink(&self, sink: Arc<dyn StreamPort>) {
        *self.sink.write().unwrap() = Some(sink);
    }

    pub fn is_enabled(&self) -> bool {
        self.sink.read().unwrap().is_some()
    }

    pub fn publish(&self, event: &ModelStreamEvent) {
        let sink = self.sink.read().unwrap().clone();
        if let Some(sink) = sink {
            sink.on_model_event(event);
        }
    }
}
