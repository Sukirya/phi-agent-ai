use std::path::PathBuf;

use serde::Serialize;
use tauri::State;

use crate::app::AppStateHandle;
use crate::contracts::ports::{
    ApproverPort, ModelPort, ModelStatus, RunStatus, RuntimePort, SessionPort, StartRunRequest,
};
use crate::domain::message::{ContentBlock, Role};
use crate::domain::view::{PendingApprovalView, SessionSummary, SessionView};
use crate::kernel::lifecycle::RunMode;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub node_id: String,
    pub mode: String,
    pub mode_reason: String,
    pub model: ModelStatus,
    pub workspace: Option<String>,
    pub sessions: Vec<SessionSummary>,
    pub last_session_id: Option<String>,
    pub max_steps: u64,
    pub max_cost_usd: f64,
    pub max_wall_secs: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    pub session: SessionView,
    pub pending_approvals: Vec<PendingApprovalView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResult {
    pub run_id: String,
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[tauri::command]
pub fn get_bootstrap(state: State<'_, AppStateHandle>) -> Result<Bootstrap, String> {
    let app = state.0.clone();
    let mut sessions = app.session.list_sessions();
    if sessions.is_empty() {
        app.session.create_session("新会话")?;
        sessions = app.session.list_sessions();
    }
    let model = app
        .services
        .model()
        .map(|model| model.status())
        .unwrap_or(ModelStatus {
            provider: "none".to_string(),
            model: "none".to_string(),
            configured: false,
        });
    Ok(Bootstrap {
        node_id: app.node_id.clone(),
        mode: app.kernel.mode().as_str().to_string(),
        mode_reason: app.kernel.mode_reason(),
        model,
        workspace: app.config.snapshot().workspace,
        last_session_id: sessions.first().map(|session| session.id.clone()),
        sessions,
        max_steps: env_u64("PHI_MAX_STEPS", 20),
        max_cost_usd: env_f64("PHI_MAX_COST_USD", 0.50),
        max_wall_secs: env_u64("PHI_MAX_WALL_SECS", 600),
    })
}

#[tauri::command]
pub fn set_workspace(state: State<'_, AppStateHandle>, path: String) -> Result<String, String> {
    let app = state.0.clone();
    let canonical = PathBuf::from(path.trim())
        .canonicalize()
        .map_err(|e| format!("路径不可用: {e}"))?;
    if !canonical.is_dir() {
        return Err("工作区必须是目录".to_string());
    }
    let display = canonical.to_string_lossy().to_string();
    app.config
        .update(|config| config.workspace = Some(display.clone()));
    Ok(display)
}

#[tauri::command]
pub fn create_session(
    state: State<'_, AppStateHandle>,
    title: Option<String>,
) -> Result<SessionSummary, String> {
    state
        .0
        .session
        .create_session(title.as_deref().unwrap_or("新会话"))
}

#[tauri::command]
pub fn get_session(
    state: State<'_, AppStateHandle>,
    session_id: String,
) -> Result<SessionDetail, String> {
    let app = state.0.clone();
    let session = app
        .session
        .get_session(&session_id)
        .ok_or_else(|| format!("未知会话: {session_id}"))?;
    let pending_approvals = app.approver.pending_for_session(&session_id);
    Ok(SessionDetail {
        session,
        pending_approvals,
    })
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppStateHandle>,
    session_id: String,
    text: String,
) -> Result<SendResult, String> {
    let app = state.0.clone();
    if app.kernel.mode() != RunMode::Normal {
        return Err(format!(
            "节点处于 {} 模式：{}",
            app.kernel.mode().as_str(),
            app.kernel.mode_reason()
        ));
    }
    let text = text.trim();
    if text.is_empty() {
        return Err("消息为空".to_string());
    }
    let workspace = app
        .config
        .snapshot()
        .workspace
        .ok_or_else(|| "请先设置工作区".to_string())?;
    let workspace = PathBuf::from(workspace);
    if !workspace.is_dir() {
        return Err("工作区不存在，请重新设置".to_string());
    }

    if let Some(active) = app.session.active_run(&session_id) {
        return Err(format!("该会话仍有运行中的任务: {active}"));
    }

    let turn = app.session.start_turn(&session_id)?;
    app.session.append_message(
        &session_id,
        Some(&turn.turn_id),
        Role::User,
        vec![ContentBlock::text(text)],
    )?;

    let run_id = app.runtime.start_run(StartRunRequest {
        session_id,
        turn_id: turn.turn_id,
        workspace,
    })?;
    Ok(SendResult { run_id })
}

#[tauri::command]
pub fn approve(
    state: State<'_, AppStateHandle>,
    approval_id: String,
    approved: bool,
    note: Option<String>,
) -> Result<(), String> {
    if state.0.approver.resolve(&approval_id, approved, note) {
        Ok(())
    } else {
        Err("审批不存在或已处理".to_string())
    }
}

#[tauri::command]
pub fn cancel_run(
    state: State<'_, AppStateHandle>,
    run_id: String,
) -> Result<bool, String> {
    Ok(state.0.runtime.cancel(&run_id, "用户取消"))
}

#[tauri::command]
pub fn set_api_key(
    state: State<'_, AppStateHandle>,
    key: String,
) -> Result<ModelStatus, String> {
    match state.0.services.model() {
        Some(model) => Ok(model.set_api_key(&key)),
        None => Err("模型网关不可用".to_string()),
    }
}

#[tauri::command]
pub fn get_model_status(state: State<'_, AppStateHandle>) -> Result<ModelStatus, String> {
    state
        .0
        .services
        .model()
        .map(|model| model.status())
        .ok_or_else(|| "模型网关不可用".to_string())
}

#[tauri::command]
pub fn get_run_status(
    state: State<'_, AppStateHandle>,
    run_id: String,
) -> Option<RunStatus> {
    state.0.runtime.status(&run_id)
}
