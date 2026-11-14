use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::message::{ContentBlock, Role};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: usize,
    pub last_run_state: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    pub id: String,
    pub turn_id: Option<String>,
    pub role: Role,
    pub ts: String,
    pub blocks: Vec<ContentBlock>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub id: String,
    pub task_id: String,
    pub turn_id: String,
    pub state: String,
    pub steps: u64,
    pub max_steps: u64,
    pub cost_usd: f64,
    pub max_cost_usd: f64,
    pub max_wall_secs: u64,
    /// True when the agent may shape the workbench view for this run.
    pub workbench_mode: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub pending_approval_id: Option<String>,
    pub error: Option<String>,
    pub stop_confirmation: Option<String>,
    pub started_at: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub id: String,
    pub owner_node_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<MessageView>,
    pub runs: Vec<RunView>,
    /// Latest workbench view tree for this session, if the agent has set one.
    #[serde(default)]
    pub ui_view: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApprovalView {
    pub id: String,
    pub session_id: String,
    pub run_id: String,
    pub invocation_id: String,
    pub tool_id: String,
    pub args_preview: Value,
    pub action_digest: String,
    pub risk: String,
    pub requested_at: String,
    pub expires_at: String,
}
