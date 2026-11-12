use std::sync::Arc;

use tauri::{AppHandle, Manager};

use crate::tauri_sink::{TauriEventSink, TauriStreamSink};
use phi_i18n as i18n;
use phi_node::{Node, NodeConfig, compose};

pub struct AppStateHandle(pub Arc<Node>);

/// Desktop composition: resolve platform paths and register shell sinks.
/// All domain wiring lives in `node::compose`, shared with headless mode.
pub fn compose(app: AppHandle) -> Result<Arc<Node>, String> {
    let data_dir = app.path().app_data_dir().map_err(|error| {
        i18n::t_args(
            "Unable to locate the application data directory: {error}",
            &[("error", error.to_string())],
        )
    })?;
    let fallback_workspace = app
        .path()
        .document_dir()
        .ok()
        .map(|documents| documents.join("phi-workspace"));
    compose(NodeConfig {
        data_dir,
        workspace: None,
        fallback_workspace,
        event_sinks: vec![Arc::new(TauriEventSink::new(app.clone()))],
        stream_sink: Some(Arc::new(TauriStreamSink::new(app))),
        language: None,
        writable: true,
    })
}
