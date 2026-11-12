use tauri::State;

use crate::app::AppStateHandle;
use phi_contracts::ports::{ModelStatus, RunStatus};
use phi_contracts::provider::{ProviderInput, ProviderTestView, ProviderView};
use phi_domain::view::SessionSummary;
use phi_node::{Bootstrap, SendResult, SessionDetail};

#[tauri::command]
pub fn get_bootstrap(state: State<'_, AppStateHandle>) -> Result<Bootstrap, String> {
    state.0.bootstrap()
}

#[tauri::command]
pub fn set_language(state: State<'_, AppStateHandle>, language: String) -> Result<String, String> {
    state.0.set_language(&language)
}

#[tauri::command]
pub fn set_workspace(state: State<'_, AppStateHandle>, path: String) -> Result<String, String> {
    state.0.set_workspace(&path)
}

#[tauri::command]
pub fn create_session(
    state: State<'_, AppStateHandle>,
    title: Option<String>,
) -> Result<SessionSummary, String> {
    state.0.create_session(title.as_deref())
}

#[tauri::command]
pub fn get_session(
    state: State<'_, AppStateHandle>,
    session_id: String,
) -> Result<SessionDetail, String> {
    state.0.get_session(&session_id)
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppStateHandle>,
    session_id: String,
    text: String,
    provider_id: Option<String>,
    ui: Option<bool>,
) -> Result<SendResult, String> {
    let run_id = state.0.send_message_full(
        &session_id,
        &text,
        provider_id.as_deref(),
        Default::default(),
        ui.unwrap_or(false),
    )?;
    Ok(SendResult { run_id })
}

#[tauri::command]
pub fn approve(
    state: State<'_, AppStateHandle>,
    approval_id: String,
    approved: bool,
    note: Option<String>,
) -> Result<(), String> {
    state.0.approve(&approval_id, approved, note)
}

#[tauri::command]
pub fn cancel_run(state: State<'_, AppStateHandle>, run_id: String) -> Result<bool, String> {
    Ok(state.0.cancel_run(&run_id))
}

#[tauri::command]
pub fn set_api_key(state: State<'_, AppStateHandle>, key: String) -> Result<ModelStatus, String> {
    state.0.set_api_key(&key)
}

#[tauri::command]
pub fn get_model_status(state: State<'_, AppStateHandle>) -> Result<ModelStatus, String> {
    Ok(state.0.model_status())
}

#[tauri::command]
pub fn get_run_status(state: State<'_, AppStateHandle>, run_id: String) -> Option<RunStatus> {
    state.0.run_status(&run_id)
}

// ─────────────────────────── providers (settings surface) ───────────────────

#[tauri::command]
pub fn list_providers(state: State<'_, AppStateHandle>) -> Vec<ProviderView> {
    state.0.provider_views()
}

#[tauri::command]
pub fn save_provider(
    state: State<'_, AppStateHandle>,
    input: ProviderInput,
) -> Result<ProviderView, String> {
    state.0.save_provider(input)
}

#[tauri::command]
pub fn remove_provider(
    state: State<'_, AppStateHandle>,
    provider_id: String,
) -> Result<bool, String> {
    state.0.remove_provider(&provider_id)
}

#[tauri::command]
pub fn set_active_provider(
    state: State<'_, AppStateHandle>,
    provider_id: String,
) -> Result<(), String> {
    state.0.set_active_provider(&provider_id)
}

#[tauri::command]
pub async fn test_provider(
    state: State<'_, AppStateHandle>,
    provider_id: Option<String>,
) -> Result<ProviderTestView, String> {
    Ok(state.0.test_provider(provider_id.as_deref()).await)
}

#[tauri::command]
pub async fn fetch_models(
    state: State<'_, AppStateHandle>,
    provider_id: Option<String>,
) -> Result<Vec<String>, String> {
    state.0.fetch_models(provider_id.as_deref()).await
}
