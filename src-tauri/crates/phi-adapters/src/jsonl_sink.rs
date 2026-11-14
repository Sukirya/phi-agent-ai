//! Headless shell sinks.
//!
//! Every durable event is written as one JSON line on stdout. Logs go to
//! stderr (see `adapters::system::StderrLog`), so a test can parse stdout
//! without filtering human text.

use std::io::{Write, stdout};

use phi_contracts::stream::{ModelStreamEvent, StreamPort};
use phi_kernel::event_bus::{EventRecord, EventSink};

pub struct JsonlEventSink {
    enabled: bool,
}

impl JsonlEventSink {
    pub fn new() -> Self {
        Self { enabled: true }
    }

    /// Suppress event lines while keeping the final result line.
    pub fn quiet() -> Self {
        Self { enabled: false }
    }
}

impl Default for JsonlEventSink {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSink for JsonlEventSink {
    fn on_event(&self, record: &EventRecord) {
        if !self.enabled {
            return;
        }
        let line = serde_json::json!({
            "type": "phi.event",
            "event": record,
        });
        write_line(&line);
    }
}

pub struct JsonlStreamSink;

impl StreamPort for JsonlStreamSink {
    fn on_model_event(&self, event: &ModelStreamEvent) {
        let line = serde_json::json!({
            "type": "phi.model_stream",
            "event": event,
        });
        write_line(&line);
    }
}

fn write_line(value: &serde_json::Value) {
    if let Ok(text) = serde_json::to_string(value) {
        let mut out = stdout().lock();
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }
}
