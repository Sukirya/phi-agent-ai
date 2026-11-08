use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::contracts::ports::{ApprovalOutcome, ApprovalSpec, ApproverPort};
use crate::contracts::SLOT_APPROVAL_DECIDE;
use crate::domain::events::DomainEvent;
use crate::domain::view::PendingApprovalView;
use crate::kernel::event_bus::EventBus;
use crate::kernel::lifecycle::{Plugin, PluginContext};
use crate::kernel::ports::{ClockPort, IdPort};
use crate::kernel::registry::Contribution;

struct Pending {
    view: PendingApprovalView,
    sender: Option<oneshot::Sender<ApprovalOutcome>>,
    receiver: Option<oneshot::Receiver<ApprovalOutcome>>,
    resolved: Option<ApprovalOutcome>,
    expires_at: Instant,
}

/// Human approver. Auxiliary-model approval is a post-MVP item; the port is
/// already shaped for it.
pub struct HumanApprover {
    bus: Arc<EventBus>,
    ids: Arc<dyn IdPort>,
    clock: Arc<dyn ClockPort>,
    pending: Mutex<HashMap<String, Pending>>,
}

impl HumanApprover {
    pub fn new(bus: Arc<EventBus>, ids: Arc<dyn IdPort>, clock: Arc<dyn ClockPort>) -> Self {
        Self {
            bus,
            ids,
            clock,
            pending: Mutex::new(HashMap::new()),
        }
    }
}

impl Plugin for HumanApprover {
    fn id(&self) -> &'static str {
        "plugin-approver"
    }

    fn activate(&self, _ctx: &PluginContext) -> Result<Vec<Contribution>, String> {
        Ok(vec![Contribution {
            plugin_id: self.id().to_string(),
            slot: SLOT_APPROVAL_DECIDE.to_string(),
            id: "human".to_string(),
        }])
    }
}

#[async_trait]
impl ApproverPort for HumanApprover {
    fn request(&self, spec: ApprovalSpec) -> Result<PendingApprovalView, String> {
        let approval_id = self.ids.new_id("appr");
        let requested_at = self.clock.now_rfc3339();
        let expires_at =
            (chrono::Utc::now() + chrono::Duration::seconds(spec.expires_in_secs as i64))
                .to_rfc3339();
        let view = PendingApprovalView {
            id: approval_id.clone(),
            session_id: spec.session_id.clone(),
            run_id: spec.run_id.clone(),
            invocation_id: spec.invocation_id.clone(),
            tool_id: spec.tool_id.clone(),
            args_preview: spec.args_preview.clone(),
            action_digest: spec.action_digest.clone(),
            risk: spec.risk.clone(),
            requested_at,
            expires_at: expires_at.clone(),
        };
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(
                approval_id.clone(),
                Pending {
                    view: view.clone(),
                    sender: Some(sender),
                    receiver: Some(receiver),
                    resolved: None,
                    expires_at: Instant::now() + Duration::from_secs(spec.expires_in_secs),
                },
            );
        }

        if let Err(error) = self.bus.publish(
            &spec.session_id,
            "plugin-approver",
            &DomainEvent::ApprovalRequested {
                approval_id: approval_id.clone(),
                invocation_id: spec.invocation_id,
                run_id: spec.run_id,
                tool_id: spec.tool_id,
                action_digest: spec.action_digest,
                risk: spec.risk,
                args_preview: spec.args_preview,
                expires_at,
            },
        ) {
            // Never leave an unaddressable pending entry behind.
            self.pending.lock().unwrap().remove(&approval_id);
            return Err(error);
        }

        Ok(view)
    }

    async fn wait(&self, approval_id: &str, cancel: CancellationToken) -> ApprovalOutcome {
        let (receiver, remaining, session_id) = {
            let mut pending = self.pending.lock().unwrap();
            match pending.get_mut(approval_id) {
                Some(entry) => {
                    if let Some(outcome) = entry.resolved.clone() {
                        let session_id = entry.view.session_id.clone();
                        pending.remove(approval_id);
                        return finish_expired(&session_id, approval_id, outcome, &self.bus);
                    }
                    let remaining = entry.expires_at.saturating_duration_since(Instant::now());
                    let session_id = entry.view.session_id.clone();
                    (entry.receiver.take(), remaining, session_id)
                }
                None => {
                    return ApprovalOutcome {
                        status: "Expired".to_string(),
                        by: "system".to_string(),
                        note: Some("审批请求不存在或已过期".to_string()),
                    }
                }
            }
        };

        let receiver = match receiver {
            Some(receiver) => receiver,
            None => {
                return ApprovalOutcome {
                    status: "Expired".to_string(),
                    by: "system".to_string(),
                    note: Some("审批请求没有等待通道".to_string()),
                }
            }
        };

        let outcome = tokio::select! {
            result = receiver => match result {
                Ok(outcome) => outcome,
                Err(_) => ApprovalOutcome {
                    status: "Expired".to_string(),
                    by: "system".to_string(),
                    note: Some("审批者已退出".to_string()),
                },
            },
            _ = tokio::time::sleep(remaining) => ApprovalOutcome {
                status: "Expired".to_string(),
                by: "system".to_string(),
                note: Some("审批超时，按拒绝处理".to_string()),
            },
            _ = cancel.cancelled() => ApprovalOutcome {
                status: "Expired".to_string(),
                by: "system".to_string(),
                note: Some("运行已取消，审批失效".to_string()),
            },
        };

        {
            let mut pending = self.pending.lock().unwrap();
            pending.remove(approval_id);
        }
        finish_expired(&session_id, approval_id, outcome, &self.bus)
    }

    fn resolve(&self, approval_id: &str, approved: bool, note: Option<String>) -> bool {
        let session_id;
        let outcome = ApprovalOutcome {
            status: if approved { "Approved" } else { "Rejected" }.to_string(),
            by: "human".to_string(),
            note,
        };
        {
            let mut pending = self.pending.lock().unwrap();
            let Some(entry) = pending.get_mut(approval_id) else {
                return false;
            };
            if entry.resolved.is_some() {
                return false;
            }
            entry.resolved = Some(outcome.clone());
            session_id = entry.view.session_id.clone();
            if let Some(sender) = entry.sender.take() {
                let _ = sender.send(outcome.clone());
            }
        }
        let _ = self.bus.publish(
            &session_id,
            "plugin-approver",
            &DomainEvent::ApprovalResolved {
                approval_id: approval_id.to_string(),
                status: outcome.status,
                by: outcome.by,
            },
        );
        true
    }

    fn pending_for_session(&self, session_id: &str) -> Vec<PendingApprovalView> {
        let pending = self.pending.lock().unwrap();
        let mut views: Vec<PendingApprovalView> = pending
            .values()
            .filter(|entry| entry.resolved.is_none() && entry.view.session_id == session_id)
            .map(|entry| entry.view.clone())
            .collect();
        views.sort_by(|a, b| a.requested_at.cmp(&b.requested_at));
        views
    }
}

fn finish_expired(
    session_id: &str,
    approval_id: &str,
    outcome: ApprovalOutcome,
    bus: &EventBus,
) -> ApprovalOutcome {
    if outcome.status == "Expired" {
        let _ = bus.publish(
            session_id,
            "plugin-approver",
            &DomainEvent::ApprovalExpired {
                approval_id: approval_id.to_string(),
            },
        );
    }
    outcome
}
