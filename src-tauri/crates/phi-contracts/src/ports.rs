use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use phi_domain::message::{ChatMessage, ContentBlock, Role};
use phi_domain::view::{PendingApprovalView, RunView, SessionSummary, SessionView};

// ─────────────────────────── tools ───────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffect {
    None,
    Reversible,
    Irreversible,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub side_effect: SideEffect,
}

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub tool_id: String,
    pub input: Value,
}

#[derive(Clone, Debug)]
pub struct ToolContext {
    pub workspace: PathBuf,
    pub session_id: String,
    pub run_id: String,
    /// Unified wall-clock deadline for the run. Tools must not start new work
    /// past it and should cap their own timeouts by the remaining time.
    pub deadline: Option<Instant>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

#[async_trait]
pub trait ToolPort: Send + Sync {
    fn definitions(&self) -> Vec<ToolDefinition>;

    fn supports(&self, tool_id: &str) -> bool {
        self.definitions().iter().any(|d| d.id == tool_id)
    }

    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, String>;
}

// ─────────────────────────── model ───────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    /// Human-facing provider name (`mock` when no provider is configured).
    pub provider: String,
    pub model: String,
    pub configured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub has_key: bool,
}

#[derive(Clone, Debug)]
pub struct ModelRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDefinition>,
    pub max_output_tokens: u32,
    /// Correlation ids used for transient stream events.
    pub call_id: String,
    pub run_id: String,
    /// Whether the shell can consume incremental output. The gateway only
    /// streams when a sink is registered.
    pub stream: bool,
    /// Whether the agent may shape the workbench view for this run.
    pub workbench: bool,
}

#[derive(Clone, Debug)]
pub struct ModelResponse {
    pub blocks: Vec<ContentBlock>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub finish_reason: String,
    pub estimated_usage: bool,
}

#[derive(Clone, Debug)]
pub struct ModelError {
    pub message: String,
    pub retryable: bool,
}

#[async_trait]
pub trait ModelPort: Send + Sync {
    fn status(&self) -> ModelStatus;
    /// Empty string clears the key. Returns the new status.
    fn set_api_key(&self, key: &str) -> Result<ModelStatus, String>;
    /// Conservative reservation estimate before a call. Both the character
    /// count and the UTF-8 byte count are provided because byte-oriented
    /// estimates are required for CJK-heavy prompts.
    fn estimate_cost(&self, input_chars: usize, input_bytes: usize, max_output_tokens: u32) -> f64;
    async fn invoke(
        &self,
        req: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelResponse, ModelError>;
}

// ─────────────────────────── policy ───────────────────────────

#[derive(Clone, Debug)]
pub struct PolicyInput {
    pub tool_id: String,
    pub args: Value,
    pub workspace: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyEffect {
    Allow,
    Deny,
    Ask,
    Modify,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyDecision {
    pub effect: PolicyEffect,
    pub rationale: String,
}

pub trait PolicyPort: Send + Sync {
    fn evaluate(&self, input: &PolicyInput) -> PolicyDecision;
}

// ─────────────────────────── approval ───────────────────────────

#[derive(Clone, Debug)]
pub struct ApprovalSpec {
    pub session_id: String,
    pub run_id: String,
    pub invocation_id: String,
    pub tool_id: String,
    pub args_preview: Value,
    pub action_digest: String,
    pub risk: String,
    pub expires_in_secs: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalOutcome {
    pub status: String,
    pub by: String,
    pub note: Option<String>,
}

#[async_trait]
pub trait ApproverPort: Send + Sync {
    /// Register an approval request and emit `ApprovalRequested`.
    fn request(&self, spec: ApprovalSpec) -> Result<PendingApprovalView, String>;
    /// Wait for a resolution. Timeout / cancellation resolves as `Expired`.
    async fn wait(&self, approval_id: &str, cancel: CancellationToken) -> ApprovalOutcome;
    /// Human (or auxiliary) resolution coming from the shell.
    fn resolve(&self, approval_id: &str, approved: bool, note: Option<String>) -> bool;
    fn pending_for_session(&self, session_id: &str) -> Vec<PendingApprovalView>;
}

// ─────────────────────────── sandbox ───────────────────────────

#[derive(Clone, Debug)]
pub struct ExecOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[async_trait]
pub trait SandboxPort: Send + Sync {
    /// Resolve a workspace-relative path and reject escapes.
    fn resolve_path(&self, workspace: &Path, requested: &str) -> Result<PathBuf, String>;
    async fn exec(
        &self,
        workspace: &Path,
        command: &str,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<ExecOutput, String>;
}

// ─────────────────────────── session ───────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInfo {
    pub turn_id: String,
    pub index: u64,
}

pub trait SessionPort: Send + Sync {
    fn node_id(&self) -> String;
    fn create_session(&self, title: &str) -> Result<SessionSummary, String>;
    fn list_sessions(&self) -> Vec<SessionSummary>;
    fn get_session(&self, session_id: &str) -> Option<SessionView>;
    fn start_turn(&self, session_id: &str) -> Result<TurnInfo, String>;
    fn append_message(
        &self,
        session_id: &str,
        turn_id: Option<&str>,
        role: Role,
        blocks: Vec<ContentBlock>,
    ) -> Result<String, String>;
    fn messages(&self, session_id: &str) -> Vec<ChatMessage>;
    fn active_run(&self, session_id: &str) -> Option<String>;
    /// Find a run by id across all sessions. Shells use this after the run has
    /// left the in-memory runtime map (or when they are a different process).
    fn find_run(&self, run_id: &str) -> Option<(String, RunView)>;
}

// ─────────────────────────── context ───────────────────────────

#[derive(Clone, Debug)]
pub struct AssembledContext {
    pub system: String,
    pub messages: Vec<ChatMessage>,
}

pub trait ContextPort: Send + Sync {
    fn assemble(&self, history: Vec<ChatMessage>, workspace: &Path) -> AssembledContext;
}

// ─────────────────────────── runtime ───────────────────────────

#[derive(Clone, Debug)]
pub struct StartRunRequest {
    pub session_id: String,
    pub turn_id: String,
    pub workspace: PathBuf,
    pub limits: RunLimits,
    /// Let the agent shape the workbench view for this run.
    pub workbench: bool,
}

/// Per-run hard caps. `None` falls back to the environment defaults, so a
/// shell can tighten a run without changing node-wide configuration.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunLimits {
    pub max_steps: Option<u64>,
    pub max_cost_usd: Option<f64>,
    pub max_wall_secs: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStatus {
    pub run_id: String,
    pub session_id: String,
    pub state: String,
    pub steps: u64,
    pub max_steps: u64,
    pub cost_usd: f64,
    pub max_cost_usd: f64,
}

pub trait RuntimePort: Send + Sync {
    fn start_run(&self, req: StartRunRequest) -> Result<String, String>;
    fn cancel(&self, run_id: &str, reason: &str) -> bool;
    fn status(&self, run_id: &str) -> Option<RunStatus>;
}
