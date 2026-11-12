use tauri::{AppHandle, Emitter};

use phi_contracts::stream::{ModelStreamEvent, StreamPort};
use phi_kernel::event_bus::{EventRecord, EventSink};

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

/// Live model deltas are transient: they are emitted to the webview but are
/// never appended to the journal (the final message is the durable record).
pub struct TauriStreamSink {
    app: AppHandle,
}

impl TauriStreamSink {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl StreamPort for TauriStreamSink {
    fn on_model_event(&self, event: &ModelStreamEvent) {
        if let Ok(value) = serde_json::to_value(event) {
            let _ = self.app.emit("phi://model-stream", value);
        }
    }
}
