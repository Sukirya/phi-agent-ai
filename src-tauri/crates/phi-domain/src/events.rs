use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::message::Role;

/// Domain event catalog (MVP subset).
///
/// Events are serialized into the journal envelope's `payload` with a `type`
/// discriminator. All events on the session line use the session id as the
/// aggregate id, so a single write order gives a restorable session stream.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum DomainEvent {
    // ── session / turn ────────────────────────────────────────────────
    SessionStarted {
        session_id: String,
        owner_node_id: String,
        title: String,
    },
    SessionArchived {
        session_id: String,
    },
    TurnStarted {
        session_id: String,
        turn_id: String,
        index: u64,
    },
    TurnClosed {
        turn_id: String,
        terminal_run_id: String,
    },
    MessageAppended {
        session_id: String,
        turn_id: Option<String>,
        message_id: String,
        role: Role,
        content_ref: String,
    },

    // ── task ──────────────────────────────────────────────────────────
    TaskCreated {
        task_id: String,
        title: String,
        implicit: bool,
    },

    // ── run / step ────────────────────────────────────────────────────
    RunScheduled {
        run_id: String,
        session_id: String,
        turn_id: String,
        task_id: String,
        max_steps: u64,
        max_cost_usd: f64,
        max_wall_secs: u64,
        /// The agent may shape the workbench view for this run.
        #[serde(default)]
        workbench: bool,
    },
    RunStarted {
        run_id: String,
    },
    StepStarted {
        run_id: String,
        step_id: String,
        index: u64,
        phase: String,
    },
    StepCompleted {
        run_id: String,
        step_id: String,
    },
    RunWaitingApproval {
        run_id: String,
        approval_id: String,
    },
    RunResumed {
        run_id: String,
        approval_id: String,
    },
    RunSucceeded {
        run_id: String,
    },
    RunFailed {
        run_id: String,
        error: String,
    },
    RunAborted {
        run_id: String,
        reason: String,
        stop_confirmation: String,
    },
    RunAwaitingReconciliation {
        run_id: String,
        note: String,
    },
    RunInterrupted {
        run_id: String,
        session_id: String,
        note: String,
    },

    // ── budget ────────────────────────────────────────────────────────
    BudgetReserved {
        run_id: String,
        dimension: String,
        amount: f64,
        note: String,
    },
    BudgetSettled {
        run_id: String,
        cost_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
        estimated: bool,
    },
    BudgetExceeded {
        run_id: String,
        dimension: String,
    },

    // ── model calls ───────────────────────────────────────────────────
    ModelCallIssued {
        call_id: String,
        run_id: String,
        step_id: String,
        model: String,
        max_output_tokens: u32,
    },
    ModelCallReturned {
        call_id: String,
        run_id: String,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
        finish_reason: String,
    },
    ModelCallFailed {
        call_id: String,
        run_id: String,
        error: String,
    },

    // ── tools ─────────────────────────────────────────────────────────
    ToolInvokeRequested {
        invocation_id: String,
        run_id: String,
        step_id: String,
        tool_id: String,
        args_preview: Value,
        action_digest: String,
    },
    ToolInvokeDecided {
        invocation_id: String,
        run_id: String,
        decision: String,
        rationale: String,
    },
    ToolInvokeStarted {
        invocation_id: String,
        run_id: String,
    },
    ToolInvokeCompleted {
        invocation_id: String,
        run_id: String,
        result_ref: String,
        is_error: bool,
        duration_ms: u64,
    },
    ToolInvokeFailed {
        invocation_id: String,
        run_id: String,
        error: String,
    },
    ToolInvokeOutcomeUnknown {
        invocation_id: String,
        run_id: String,
        evidence: String,
    },

    // ── workbench ─────────────────────────────────────────────────────
    /// The latest view tree replaces the session's workbench. Views are data,
    /// not code: the shell renders them with its own component registry.
    UiViewUpdated {
        run_id: String,
        view: Value,
    },

    // ── approval ──────────────────────────────────────────────────────
    ApprovalRequested {
        approval_id: String,
        invocation_id: String,
        run_id: String,
        tool_id: String,
        action_digest: String,
        risk: String,
        args_preview: Value,
        expires_at: String,
    },
    ApprovalResolved {
        approval_id: String,
        status: String,
        by: String,
    },
    ApprovalExpired {
        approval_id: String,
    },
}
