mod adapters;
mod api;
mod app;
mod contracts;
mod domain;
mod kernel;
mod plugins;

use std::sync::Arc;

use tauri::Manager;

use app::{compose, AppStateHandle};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let state = compose(handle)
                .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
            app.manage(AppStateHandle(Arc::new(state)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            api::get_bootstrap,
            api::set_workspace,
            api::create_session,
            api::get_session,
            api::send_message,
            api::approve,
            api::cancel_run,
            api::set_api_key,
            api::get_model_status,
            api::get_run_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
