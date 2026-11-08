use tauri::{AppHandle, Emitter};

use crate::kernel::event_bus::{EventRecord, EventSink};

/// Shell-layer adapter: forwards every durable event to the webview. The UI
/// treats it as a "state changed" signal and pulls a read-model snapshot.
pub struct TauriEventSink {
    app: AppHandle,
}

impl TauriEventSink {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl EventSink for TauriEventSink {
    fn on_event(&self, record: &EventRecord) {
        if let Ok(value) = serde_json::to_value(record) {
            let _ = self.app.emit("phi://event", value);
        }
    }
}
