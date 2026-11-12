//! Desktop shell: platform paths, Tauri sinks, commands and the application
//! entry point. All domain wiring lives in `phi-node`.

mod api;
mod app;
mod tauri_sink;

use app::{AppStateHandle, compose};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let state =
                compose(handle).map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
            app.manage(AppStateHandle(state));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            api::get_bootstrap,
            api::set_language,
            api::set_workspace,
            api::create_session,
            api::get_session,
            api::send_message,
            api::approve,
            api::cancel_run,
            api::set_api_key,
            api::get_model_status,
            api::get_run_status,
            api::list_providers,
            api::save_provider,
            api::remove_provider,
            api::set_active_provider,
            api::test_provider,
            api::fetch_models,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
