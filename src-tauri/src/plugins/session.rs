use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use crate::contracts::ports::{SessionPort, TurnInfo};
use crate::contracts::SLOT_SESSION_STORE;
use crate::domain::events::DomainEvent;
use crate::domain::message::{ChatMessage, ContentBlock, Role};
use crate::domain::view::{
    MessageView, RunView, SessionSummary, SessionView,
};
use crate::kernel::event_bus::{EventBus, EventRecord, EventSink};
use crate::kernel::lifecycle::{Plugin, PluginContext};
use crate::kernel::ports::{BlobPort, ClockPort, IdPort};
use crate::kernel::registry::Contribution;

#[derive(Clone)]
struct StoredMessage {
    id: String,
    turn_id: Option<String>,
    role: Role,
    ts: String,
    content_ref: String,
}

#[derive(Clone)]
struct RunRecord {
    id: String,
    task_id: String,
    turn_id: String,
    state: String,
    steps: u64,
    max_steps: u64,
    cost_usd: f64,
    max_cost_usd: f64,
    max_wall_secs: u64,
    input_tokens: u64,
    output_tokens: u64,
    pending_approval_id: Option<String>,
    error: Option<String>,
    stop_confirmation: Option<String>,
    started_at: Option<String>,
    updated_at: String,
}

impl RunRecord {
    fn is_active(&self) -> bool {
        matches!(
            self.state.as_str(),
            "Scheduled" | "Running" | "WaitingApproval"
        )
    }

    fn is_terminal(&self) -> bool {
        matches!(
            self.state.as_str(),
            "Succeeded" | "Failed" | "Aborted" | "AwaitingReconciliation"
        )
    }
}

#[derive(Clone)]
struct StoredSession {
    id: String,
    owner_node_id: String,
    title: String,
    created_at: String,
    updated_at: String,
    messages: Vec<StoredMessage>,
    runs: HashMap<String, RunRecord>,
    run_order: Vec<String>,
    turn_count: u64,
}

#[derive(Default)]
struct Projection {
    sessions: HashMap<String, StoredSession>,
}

/// Session read model, rebuilt by folding the journal.
///
/// Session is the consistency boundary for v1 (ADR-0011): run/tool/approval
/// events on a session line all use the session id as aggregate id.
pub struct SessionPlugin {
    bus: Arc<EventBus>,
    ids: Arc<dyn IdPort>,
    clock: Arc<dyn ClockPort>,
    blob: Arc<dyn BlobPort>,
    node_id: String,
    state: RwLock<Projection>,
}

impl SessionPlugin {
    pub fn new(
        bus: Arc<EventBus>,
        ids: Arc<dyn IdPort>,
        clock: Arc<dyn ClockPort>,
        blob: Arc<dyn BlobPort>,
        node_id: String,
    ) -> Self {
        Self {
            bus,
            ids,
            clock,
            blob,
            node_id,
            state: RwLock::new(Projection::default()),
        }
    }

    pub fn load(&self, records: &[EventRecord]) {
        let mut projection = self.state.write().unwrap();
        for record in records {
            apply_record(&mut projection, record);
        }
    }

    /// On startup, a run without a terminal event means we crashed mid-flight.
    /// Never auto-retry: mark it for human reconciliation (I-5-4 / I-3-8).
    pub fn recover_interrupted(&self) -> usize {
        let mut pending: Vec<(String, String, String)> = Vec::new();
        {
            let projection = self.state.read().unwrap();
            for session in projection.sessions.values() {
                for run_id in &session.run_order {
                    if let Some(run) = session.runs.get(run_id) {
                        if !run.is_terminal() {
                            pending.push((
                                session.id.clone(),
                                run.id.clone(),
                                run.state.clone(),
                            ));
                        }
                    }
                }
            }
        }

        let mut repaired_sessions: Vec<String> = Vec::new();
        for (session_id, run_id, state) in &pending {
            let note = if state == "WaitingApproval" {
                "进程重启前正在等待审批，动作尚未执行。请核对后重新发起。"
            } else {
                "进程重启前运行未正常结束，副作用结果未知。请检查工作区后决定是否重试。"
            };
            let _ = self.bus.publish(
                session_id,
                "plugin-session",
                &DomainEvent::RunInterrupted {
                    run_id: run_id.clone(),
                    session_id: session_id.clone(),
                    note: note.to_string(),
                },
            );
            if !repaired_sessions.contains(session_id) {
                repaired_sessions.push(session_id.clone());
            }
        }

        // A crash between saving the assistant message and saving its tool
        // results leaves an unpaired tool_use in history. Repair it before the
        // next model call, otherwise the provider rejects the request.
        for session_id in &repaired_sessions {
            let _ = self.repair_unpaired_tool_uses(session_id);
        }
        pending.len()
    }

    fn repair_unpaired_tool_uses(&self, session_id: &str) -> Result<usize, String> {
        let stored = {
            let projection = self.state.read().unwrap();
            match projection.sessions.get(session_id) {
                Some(session) => session.messages.clone(),
                None => return Ok(0),
            }
        };

        let mut resolved: HashSet<String> = HashSet::new();
        for message in &stored {
            for block in self.resolve_blocks(&message.content_ref) {
                if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                    resolved.insert(tool_use_id);
                }
            }
        }

        let mut missing: Vec<String> = Vec::new();
        for message in &stored {
            for block in self.resolve_blocks(&message.content_ref) {
                if let ContentBlock::ToolUse { id, .. } = block {
                    if !resolved.contains(&id) && !missing.contains(&id) {
                        missing.push(id);
                    }
                }
            }
        }
        if missing.is_empty() {
            return Ok(0);
        }

        let blocks = missing
            .iter()
            .map(|tool_use_id| {
                ContentBlock::tool_result(
                    tool_use_id.clone(),
                    "Run interrupted before the tool result was recorded; the action may have executed. The run needs manual reconciliation and will not be retried automatically.",
                    true,
                )
            })
            .collect();
        self.append_message(session_id, None, Role::User, blocks)?;
        Ok(missing.len())
    }

    fn summary_of(session: &StoredSession) -> SessionSummary {
        SessionSummary {
            id: session.id.clone(),
            title: session.title.clone(),
            created_at: session.created_at.clone(),
            updated_at: session.updated_at.clone(),
            message_count: session.messages.len(),
            last_run_state: session
                .run_order
                .last()
                .and_then(|id| session.runs.get(id))
                .map(|r| r.state.clone()),
        }
    }
}

impl SessionPort for SessionPlugin {
    fn node_id(&self) -> String {
        self.node_id.clone()
    }

    fn create_session(&self, title: &str) -> Result<SessionSummary, String> {
        let session_id = self.ids.new_id("sess");
        let title = title.trim();
        let title = if title.is_empty() { "新会话" } else { title };
        self.bus.publish(
            &session_id,
            "plugin-session",
            &DomainEvent::SessionStarted {
                session_id: session_id.clone(),
                owner_node_id: self.node_id.clone(),
                title: title.to_string(),
            },
        )?;
        self.list_sessions()
            .into_iter()
            .find(|s| s.id == session_id)
            .ok_or_else(|| "session projection did not materialize".to_string())
    }

    fn list_sessions(&self) -> Vec<SessionSummary> {
        let projection = self.state.read().unwrap();
        let mut sessions: Vec<SessionSummary> = projection
            .sessions
            .values()
            .map(Self::summary_of)
            .collect();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions
    }

    fn get_session(&self, session_id: &str) -> Option<SessionView> {
        let stored = {
            let projection = self.state.read().unwrap();
            projection.sessions.get(session_id)?.clone()
        };

        let messages = stored
            .messages
            .iter()
            .map(|m| MessageView {
                id: m.id.clone(),
                turn_id: m.turn_id.clone(),
                role: m.role,
                ts: m.ts.clone(),
                blocks: self.resolve_blocks(&m.content_ref),
            })
            .collect();

        let runs = stored
            .run_order
            .iter()
            .filter_map(|id| stored.runs.get(id))
            .map(|r| RunView {
                id: r.id.clone(),
                task_id: r.task_id.clone(),
                turn_id: r.turn_id.clone(),
                state: r.state.clone(),
                steps: r.steps,
                max_steps: r.max_steps,
                cost_usd: r.cost_usd,
                max_cost_usd: r.max_cost_usd,
                max_wall_secs: r.max_wall_secs,
                input_tokens: r.input_tokens,
                output_tokens: r.output_tokens,
                pending_approval_id: r.pending_approval_id.clone(),
                error: r.error.clone(),
                stop_confirmation: r.stop_confirmation.clone(),
                started_at: r.started_at.clone(),
                updated_at: r.updated_at.clone(),
            })
            .collect();

        Some(SessionView {
            id: stored.id.clone(),
            owner_node_id: stored.owner_node_id.clone(),
            title: stored.title.clone(),
            created_at: stored.created_at.clone(),
            updated_at: stored.updated_at.clone(),
            messages,
            runs,
        })
    }

    fn start_turn(&self, session_id: &str) -> Result<TurnInfo, String> {
        let index = {
            let projection = self.state.read().unwrap();
            let session = projection
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("unknown session: {session_id}"))?;
            session.turn_count + 1
        };
        let turn_id = self.ids.new_id("turn");
        self.bus.publish(
            session_id,
            "plugin-session",
            &DomainEvent::TurnStarted {
                session_id: session_id.to_string(),
                turn_id: turn_id.clone(),
                index,
            },
        )?;
        Ok(TurnInfo { turn_id, index })
    }

    fn append_message(
        &self,
        session_id: &str,
        turn_id: Option<&str>,
        role: Role,
        blocks: Vec<ContentBlock>,
    ) -> Result<String, String> {
        {
            let projection = self.state.read().unwrap();
            if !projection.sessions.contains_key(session_id) {
                return Err(format!("unknown session: {session_id}"));
            }
        }
        let raw = serde_json::to_vec(&blocks).map_err(|e| e.to_string())?;
        let content_ref = self.blob.put(&raw)?;
        let message_id = self.ids.new_id("msg");
        self.bus.publish(
            session_id,
            "plugin-session",
            &DomainEvent::MessageAppended {
                session_id: session_id.to_string(),
                turn_id: turn_id.map(|s| s.to_string()),
                message_id: message_id.clone(),
                role,
                content_ref,
            },
        )?;
        Ok(message_id)
    }

    fn messages(&self, session_id: &str) -> Vec<ChatMessage> {
        let stored = {
            let projection = self.state.read().unwrap();
            match projection.sessions.get(session_id) {
                Some(session) => session.messages.clone(),
                None => return Vec::new(),
            }
        };
        stored
            .iter()
            .map(|m| ChatMessage {
                role: m.role,
                blocks: self.resolve_blocks(&m.content_ref),
            })
            .collect()
    }

    fn active_run(&self, session_id: &str) -> Option<String> {
        let projection = self.state.read().unwrap();
        let session = projection.sessions.get(session_id)?;
        session
            .run_order
            .iter()
            .filter_map(|id| session.runs.get(id))
            .find(|r| r.is_active())
            .map(|r| r.id.clone())
    }
}

impl SessionPlugin {
    fn resolve_blocks(&self, content_ref: &str) -> Vec<ContentBlock> {
        match self.blob.get(content_ref) {
            Ok(raw) => serde_json::from_slice::<Vec<ContentBlock>>(&raw).unwrap_or_else(|_| {
                vec![ContentBlock::text("（内容无法解析）")]
            }),
            Err(_) => vec![ContentBlock::text("（内容不可读）")],
        }
    }
}

impl EventSink for SessionPlugin {
    fn on_event(&self, record: &EventRecord) {
        let mut projection = self.state.write().unwrap();
        apply_record(&mut projection, record);
    }
}

impl Plugin for SessionPlugin {
    fn id(&self) -> &'static str {
        "plugin-session"
    }

    fn activate(&self, _ctx: &PluginContext) -> Result<Vec<Contribution>, String> {
        Ok(vec![Contribution {
            plugin_id: self.id().to_string(),
            slot: SLOT_SESSION_STORE.to_string(),
            id: "default".to_string(),
        }])
    }
}

fn apply_record(projection: &mut Projection, record: &EventRecord) {
    let event: DomainEvent = match serde_json::from_value(record.payload.clone()) {
        Ok(event) => event,
        Err(_) => return,
    };

    match event {
        DomainEvent::SessionStarted {
            session_id,
            owner_node_id,
            title,
        } => {
            projection.sessions.entry(session_id.clone()).or_insert_with(|| StoredSession {
                id: session_id,
                owner_node_id,
                title,
                created_at: record.ts.clone(),
                updated_at: record.ts.clone(),
                messages: Vec::new(),
                runs: HashMap::new(),
                run_order: Vec::new(),
                turn_count: 0,
            });
        }
        DomainEvent::TurnStarted { session_id, index, .. } => {
            if let Some(session) = projection.sessions.get_mut(&session_id) {
                session.turn_count = session.turn_count.max(index);
                session.updated_at = record.ts.clone();
            }
        }
        DomainEvent::MessageAppended {
            session_id,
            turn_id,
            message_id,
            role,
            content_ref,
        } => {
            if let Some(session) = projection.sessions.get_mut(&session_id) {
                session.messages.push(StoredMessage {
                    id: message_id,
                    turn_id,
                    role,
                    ts: record.ts.clone(),
                    content_ref,
                });
                session.updated_at = record.ts.clone();
            }
        }
        DomainEvent::RunScheduled {
            run_id,
            session_id,
            turn_id,
            task_id,
            max_steps,
            max_cost_usd,
            max_wall_secs,
        } => {
            if let Some(session) = projection.sessions.get_mut(&session_id) {
                if !session.runs.contains_key(&run_id) {
                    session.run_order.push(run_id.clone());
                }
                session.runs.insert(
                    run_id.clone(),
                    RunRecord {
                        id: run_id,
                        task_id,
                        turn_id,
                        state: "Scheduled".to_string(),
                        steps: 0,
                        max_steps,
                        cost_usd: 0.0,
                        max_cost_usd,
                        max_wall_secs,
                        input_tokens: 0,
                        output_tokens: 0,
                        pending_approval_id: None,
                        error: None,
                        stop_confirmation: None,
                        started_at: None,
                        updated_at: record.ts.clone(),
                    },
                );
                session.updated_at = record.ts.clone();
            }
        }
        DomainEvent::RunStarted { run_id }
        | DomainEvent::StepCompleted { run_id, .. }
        | DomainEvent::RunResumed { run_id, .. } => {
            update_run(projection, record, &run_id, |run| match record.payload["type"].as_str() {
                Some("RunStarted") => {
                    run.state = "Running".to_string();
                    run.started_at = Some(record.ts.clone());
                }
                Some("RunResumed") => {
                    run.state = "Running".to_string();
                    run.pending_approval_id = None;
                }
                _ => {}
            });
        }
        DomainEvent::StepStarted { run_id, index, .. } => {
            update_run(projection, record, &run_id, |run| {
                run.steps = index;
                if run.state == "Scheduled" {
                    run.state = "Running".to_string();
                }
            });
        }
        DomainEvent::RunWaitingApproval {
            run_id,
            approval_id,
        } => {
            update_run(projection, record, &run_id, |run| {
                run.state = "WaitingApproval".to_string();
                run.pending_approval_id = Some(approval_id);
            });
        }
        DomainEvent::BudgetSettled {
            run_id,
            cost_usd,
            input_tokens,
            output_tokens,
            ..
        } => {
            update_run(projection, record, &run_id, |run| {
                run.cost_usd += cost_usd;
                run.input_tokens += input_tokens;
                run.output_tokens += output_tokens;
            });
        }
        DomainEvent::RunSucceeded { run_id } => {
            update_run(projection, record, &run_id, |run| {
                run.state = "Succeeded".to_string();
                run.pending_approval_id = None;
            });
        }
        DomainEvent::RunFailed { run_id, error } => {
            update_run(projection, record, &run_id, |run| {
                run.state = "Failed".to_string();
                run.error = Some(error);
                run.pending_approval_id = None;
            });
        }
        DomainEvent::RunAborted {
            run_id,
            reason,
            stop_confirmation,
        } => {
            update_run(projection, record, &run_id, |run| {
                run.state = "Aborted".to_string();
                run.error = Some(reason);
                run.stop_confirmation = Some(stop_confirmation);
                run.pending_approval_id = None;
            });
        }
        DomainEvent::RunAwaitingReconciliation { run_id, note } => {
            update_run(projection, record, &run_id, |run| {
                run.state = "AwaitingReconciliation".to_string();
                run.error = Some(note);
                run.pending_approval_id = None;
            });
        }
        DomainEvent::RunInterrupted {
            run_id,
            session_id,
            note,
        } => {
            if let Some(session) = projection.sessions.get_mut(&session_id) {
                if let Some(run) = session.runs.get_mut(&run_id) {
                    run.state = "AwaitingReconciliation".to_string();
                    run.error = Some(note);
                    run.pending_approval_id = None;
                    run.updated_at = record.ts.clone();
                }
                session.updated_at = record.ts.clone();
            }
        }
        _ => {
            // Tool/model/approval leaf events only bump activity timestamps.
            if let Some(session) = projection.sessions.get_mut(&record.aggregate_id) {
                session.updated_at = record.ts.clone();
            }
        }
    }
}

fn update_run(
    projection: &mut Projection,
    record: &EventRecord,
    run_id: &str,
    mutate: impl FnOnce(&mut RunRecord),
) {
    if let Some(session) = projection.sessions.get_mut(&record.aggregate_id) {
        if let Some(run) = session.runs.get_mut(run_id) {
            mutate(run);
            run.updated_at = record.ts.clone();
        }
        session.updated_at = record.ts.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::fs_blob::FsBlobStore;
    use crate::adapters::fs_journal::JsonlJournal;
    use crate::adapters::system::{StderrLog, SystemClock, UuidIds};
    use serde_json::json;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("phi-test-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn folds_session_and_run_state() {
        let dir = temp_dir("session");
        let bus = Arc::new(EventBus::new(
            Arc::new(JsonlJournal::new(dir.join("events.jsonl"))),
            Arc::new(SystemClock::new()),
            Arc::new(UuidIds),
            Arc::new(StderrLog),
        ));
        let plugin = Arc::new(SessionPlugin::new(
            bus.clone(),
            Arc::new(UuidIds),
            Arc::new(SystemClock::new()),
            Arc::new(FsBlobStore::new(dir.join("blobs"))),
            "node-test".to_string(),
        ));
        bus.subscribe(plugin.clone());

        let session = plugin.create_session("t").unwrap();
        let turn = plugin.start_turn(&session.id).unwrap();
        plugin
            .append_message(&session.id, Some(&turn.turn_id), Role::User, vec![ContentBlock::text("hi")])
            .unwrap();
        bus.publish(
            &session.id,
            "test",
            &json!({
                "type": "RunScheduled",
                "runId": "run-1",
                "sessionId": session.id,
                "turnId": turn.turn_id,
                "taskId": "task-1",
                "maxSteps": 5,
                "maxCostUsd": 0.5,
                "maxWallSecs": 60
            }),
        )
        .unwrap();
        bus.publish(
            &session.id,
            "test",
            &json!({ "type": "RunSucceeded", "runId": "run-1" }),
        )
        .unwrap();

        let view = plugin.get_session(&session.id).unwrap();
        assert_eq!(view.messages.len(), 1);
        assert_eq!(view.messages[0].blocks.len(), 1);
        assert_eq!(view.runs[0].state, "Succeeded");
    }

    #[test]
    fn repairs_unpaired_tool_use_after_interruption() {
        let dir = temp_dir("session-repair");
        let bus = Arc::new(EventBus::new(
            Arc::new(JsonlJournal::new(dir.join("events.jsonl"))),
            Arc::new(SystemClock::new()),
            Arc::new(UuidIds),
            Arc::new(StderrLog),
        ));
        let plugin = Arc::new(SessionPlugin::new(
            bus.clone(),
            Arc::new(UuidIds),
            Arc::new(SystemClock::new()),
            Arc::new(FsBlobStore::new(dir.join("blobs"))),
            "node-test".to_string(),
        ));
        bus.subscribe(plugin.clone());

        let session = plugin.create_session("t").unwrap();
        // Crash window: the assistant message was saved, its tool results were
        // not. The next model call would otherwise be rejected by the provider.
        plugin
            .append_message(
                &session.id,
                None,
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call-1".to_string(),
                    name: "fs_list".to_string(),
                    input: json!({ "path": "." }),
                }],
            )
            .unwrap();

        assert_eq!(plugin.repair_unpaired_tool_uses(&session.id).unwrap(), 1);
        let view = plugin.get_session(&session.id).unwrap();
        let last = view.messages.last().unwrap();
        assert_eq!(last.role, Role::User);
        match &last.blocks[0] {
            ContentBlock::ToolResult { tool_use_id, .. } => {
                assert_eq!(tool_use_id, "call-1")
            }
            other => panic!("expected tool_result, got {other:?}"),
        }

        // Repair must be idempotent across subsequent restarts.
        assert_eq!(plugin.repair_unpaired_tool_uses(&session.id).unwrap(), 0);
    }
}
