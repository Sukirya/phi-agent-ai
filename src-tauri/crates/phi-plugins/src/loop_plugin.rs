use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use phi_contracts::SLOT_RUNTIME_SKELETON;
use phi_contracts::ports::{
    AssembledContext, ModelRequest, PolicyEffect, PolicyInput, RunStatus, RuntimePort,
    StartRunRequest, ToolCall, ToolContext,
};
use phi_contracts::util::{
    ResourceIdentity, action_digest, args_preview, resource_fingerprint, truncate_chars,
};
use phi_domain::events::DomainEvent;
use phi_domain::message::{ChatMessage, ContentBlock, Role};
use phi_i18n as i18n;
use phi_kernel::event_bus::EventBus;
use phi_kernel::lifecycle::{Plugin, PluginContext};
use phi_kernel::ports::{BlobPort, IdPort, LogPort};
use phi_kernel::registry::Contribution;

use phi_contracts::services::Services;

struct RunControl {
    cancel: CancellationToken,
    status: Arc<Mutex<RunStatus>>,
}

pub struct LoopPlugin {
    bus: Arc<EventBus>,
    ids: Arc<dyn IdPort>,
    log: Arc<dyn LogPort>,
    blob: Arc<dyn BlobPort>,
    services: Arc<Services>,
    running: Arc<Mutex<HashMap<String, RunControl>>>,
}

impl LoopPlugin {
    pub fn new(
        bus: Arc<EventBus>,
        ids: Arc<dyn IdPort>,
        log: Arc<dyn LogPort>,
        blob: Arc<dyn BlobPort>,
        services: Arc<Services>,
    ) -> Self {
        Self {
            bus,
            ids,
            log,
            blob,
            services,
            running: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Plugin for LoopPlugin {
    fn id(&self) -> &'static str {
        "plugin-loop"
    }

    fn activate(&self, _ctx: &PluginContext) -> Result<Vec<Contribution>, String> {
        Ok(vec![Contribution {
            plugin_id: self.id().to_string(),
            slot: SLOT_RUNTIME_SKELETON.to_string(),
            id: "react".to_string(),
        }])
    }
}

impl RuntimePort for LoopPlugin {
    fn start_run(&self, req: StartRunRequest) -> Result<String, String> {
        let session = self.services.session().ok_or_else(|| {
            i18n::t("Session store is not available (session.store slot is missing)")
        })?;
        if let Some(active) = session.active_run(&req.session_id) {
            return Err(i18n::t_args(
                "This session still has a running task: {run_id}",
                &[("run_id", active)],
            ));
        }
        if !req.workspace.is_dir() {
            return Err(i18n::t("Workspace does not exist or is not a directory"));
        }

        let run_id = self.ids.new_id("run");
        let task_id = self.ids.new_id("task");
        let max_steps = req
            .limits
            .max_steps
            .unwrap_or_else(|| env_u64("PHI_MAX_STEPS", 20));
        let max_cost_usd = req
            .limits
            .max_cost_usd
            .unwrap_or_else(|| env_f64("PHI_MAX_COST_USD", 0.50));
        let max_wall_secs = req
            .limits
            .max_wall_secs
            .unwrap_or_else(|| env_u64("PHI_MAX_WALL_SECS", 600));

        self.bus.publish(
            &req.session_id,
            "plugin-loop",
            &DomainEvent::TaskCreated {
                task_id: task_id.clone(),
                title: i18n::t("Implicit task"),
                implicit: true,
            },
        )?;

        self.bus.publish(
            &req.session_id,
            "plugin-loop",
            &DomainEvent::RunScheduled {
                run_id: run_id.clone(),
                session_id: req.session_id.clone(),
                turn_id: req.turn_id.clone(),
                task_id,
                max_steps,
                max_cost_usd,
                max_wall_secs,
                ui: req.ui,
            },
        )?;

        let status = Arc::new(Mutex::new(RunStatus {
            run_id: run_id.clone(),
            session_id: req.session_id.clone(),
            state: "Scheduled".to_string(),
            steps: 0,
            max_steps,
            cost_usd: 0.0,
            max_cost_usd,
        }));
        let cancel = CancellationToken::new();
        {
            let mut running = self.running.lock().unwrap();
            running.insert(
                run_id.clone(),
                RunControl {
                    cancel: cancel.clone(),
                    status: status.clone(),
                },
            );
        }

        let ctx = RunContext {
            bus: self.bus.clone(),
            ids: self.ids.clone(),
            blob: self.blob.clone(),
            services: self.services.clone(),
            running: self.running.clone(),
            run_id: run_id.clone(),
            session_id: req.session_id,
            turn_id: req.turn_id,
            workspace: req.workspace,
            cancel,
            deadline: Instant::now() + Duration::from_secs(max_wall_secs),
            status,
            max_steps,
            max_cost_usd,
            ui_mode: req.ui,
        };
        tokio::spawn(run_loop(ctx));
        Ok(run_id)
    }

    fn cancel(&self, run_id: &str, reason: &str) -> bool {
        let running = self.running.lock().unwrap();
        if let Some(control) = running.get(run_id) {
            self.log
                .info(&format!("cancel requested for {run_id}: {reason}"));
            control.cancel.cancel();
            true
        } else {
            false
        }
    }

    fn status(&self, run_id: &str) -> Option<RunStatus> {
        self.running
            .lock()
            .unwrap()
            .get(run_id)
            .map(|control| control.status.lock().unwrap().clone())
    }
}

struct RunContext {
    bus: Arc<EventBus>,
    ids: Arc<dyn IdPort>,
    blob: Arc<dyn BlobPort>,
    services: Arc<Services>,
    running: Arc<Mutex<HashMap<String, RunControl>>>,
    run_id: String,
    session_id: String,
    turn_id: String,
    workspace: PathBuf,
    cancel: CancellationToken,
    /// Unified wall-clock deadline shared by model calls, approvals and tools.
    deadline: Instant,
    status: Arc<Mutex<RunStatus>>,
    max_steps: u64,
    max_cost_usd: f64,
    ui_mode: bool,
}

/// Generated from the built-in OpenUI chat library. Regenerate with
/// `bun run generate:openui` after upgrading the OpenUI packages.
const OPENUI_SYSTEM_PROMPT: &str = include_str!("../../../assets/openui-chat-system-prompt.txt");

enum RunOutcome {
    Succeeded,
    Failed(String),
    Aborted { reason: String, stop: String },
}

struct Ledger {
    max_cost: f64,
    spent: f64,
    reserved: f64,
}

impl Ledger {
    fn new(max_cost: f64) -> Self {
        Self {
            max_cost,
            spent: 0.0,
            reserved: 0.0,
        }
    }

    fn reserve(&mut self, amount: f64) -> bool {
        if self.spent + self.reserved + amount > self.max_cost + 1e-9 {
            false
        } else {
            self.reserved += amount;
            true
        }
    }

    fn release(&mut self, amount: f64) {
        self.reserved = (self.reserved - amount).max(0.0);
    }

    fn settle(&mut self, reserved: f64, actual: f64) {
        self.release(reserved);
        self.spent += actual;
    }
}

async fn run_loop(ctx: RunContext) {
    let outcome = run_inner(&ctx).await;

    let state = match outcome {
        RunOutcome::Succeeded => {
            let _ = ctx.bus.publish(
                &ctx.session_id,
                "plugin-loop",
                &DomainEvent::RunSucceeded {
                    run_id: ctx.run_id.clone(),
                },
            );
            "Succeeded"
        }
        RunOutcome::Failed(error) => {
            let _ = ctx.bus.publish(
                &ctx.session_id,
                "plugin-loop",
                &DomainEvent::RunFailed {
                    run_id: ctx.run_id.clone(),
                    error,
                },
            );
            "Failed"
        }
        RunOutcome::Aborted { reason, stop } => {
            let _ = ctx.bus.publish(
                &ctx.session_id,
                "plugin-loop",
                &DomainEvent::RunAborted {
                    run_id: ctx.run_id.clone(),
                    reason,
                    stop_confirmation: stop,
                },
            );
            "Aborted"
        }
    };

    {
        let mut status = ctx.status.lock().unwrap();
        status.state = state.to_string();
    }

    let _ = ctx.bus.publish(
        &ctx.session_id,
        "plugin-loop",
        &DomainEvent::TurnClosed {
            turn_id: ctx.turn_id.clone(),
            terminal_run_id: ctx.run_id.clone(),
        },
    );

    {
        let mut running = ctx.running.lock().unwrap();
        running.remove(&ctx.run_id);
    }
}

async fn run_inner(ctx: &RunContext) -> RunOutcome {
    let Some(session) = ctx.services.session() else {
        return RunOutcome::Failed(i18n::t("Session store is not available"));
    };
    let Some(model) = ctx.services.model() else {
        return RunOutcome::Failed(i18n::t("Model gateway is not available"));
    };

    set_status(ctx, "Running");
    let _ = ctx.bus.publish(
        &ctx.session_id,
        "plugin-loop",
        &DomainEvent::RunStarted {
            run_id: ctx.run_id.clone(),
        },
    );

    let mut ledger = Ledger::new(ctx.max_cost_usd);
    let tools = if ctx.ui_mode {
        Vec::new()
    } else {
        ctx.services.tool_definitions()
    };
    let max_output_tokens = env_u64("PHI_MAX_OUTPUT_TOKENS", 4096) as u32;

    for step_index in 1..=ctx.max_steps {
        if ctx.cancel.is_cancelled() {
            return aborted(
                ctx,
                &i18n::t("Cancelled by user"),
                &i18n::t("Stopped: the in-flight request was aborted."),
            );
        }
        if Instant::now() >= ctx.deadline {
            let _ = ctx.bus.publish(
                &ctx.session_id,
                "plugin-loop",
                &DomainEvent::BudgetExceeded {
                    run_id: ctx.run_id.clone(),
                    dimension: "wall_time".to_string(),
                },
            );
            return aborted(
                ctx,
                &i18n::t("Wall-clock limit exceeded"),
                &i18n::t("Stopped: the run exceeded the time limit."),
            );
        }

        let history = session.messages(&ctx.session_id);
        let mut assembled = match ctx.services.context() {
            Some(assembler) => assembler.assemble(history, &ctx.workspace),
            None => fallback_context(history, &ctx.workspace),
        };
        if ctx.ui_mode {
            assembled.system = OPENUI_SYSTEM_PROMPT.trim().to_string();
        }

        let step_id = ctx.ids.new_id("step");
        if let Err(error) = ctx.bus.publish(
            &ctx.session_id,
            "plugin-loop",
            &DomainEvent::StepStarted {
                run_id: ctx.run_id.clone(),
                step_id: step_id.clone(),
                index: step_index,
                phase: "Planning".to_string(),
            },
        ) {
            return RunOutcome::Failed(i18n::t_args(
                "Failed to record the step start: {error}",
                &[("error", error)],
            ));
        }
        set_status_progress(ctx, step_index, ledger.spent);

        let mut response = None;
        let mut attempt = 0;
        while attempt < 2 {
            attempt += 1;
            if Instant::now() >= ctx.deadline {
                let _ = ctx.bus.publish(
                    &ctx.session_id,
                    "plugin-loop",
                    &DomainEvent::BudgetExceeded {
                        run_id: ctx.run_id.clone(),
                        dimension: "wall_time".to_string(),
                    },
                );
                return aborted(
                    ctx,
                    &i18n::t("Wall-clock limit exceeded"),
                    &i18n::t("Stopped: the run exceeded the time limit."),
                );
            }
            let input_chars = estimate_chars(&assembled.system, &assembled.messages, &tools);
            let input_bytes = estimate_bytes(&assembled.system, &assembled.messages, &tools);
            let estimate = model.estimate_cost(input_chars, input_bytes, max_output_tokens);
            if !ledger.reserve(estimate) {
                let _ = ctx.bus.publish(
                    &ctx.session_id,
                    "plugin-loop",
                    &DomainEvent::BudgetExceeded {
                        run_id: ctx.run_id.clone(),
                        dimension: "cost".to_string(),
                    },
                );
                return aborted(
                    ctx,
                    &i18n::t("Cost budget exceeded (reservation failed before the call)"),
                    &i18n::t("Stopped: the budget is insufficient for the next model call."),
                );
            }
            if let Err(error) = ctx.bus.publish(
                &ctx.session_id,
                "plugin-loop",
                &DomainEvent::BudgetReserved {
                    run_id: ctx.run_id.clone(),
                    dimension: "cost".to_string(),
                    amount: estimate,
                    note: i18n::t("Reservation before model call"),
                },
            ) {
                // If the reservation cannot be made durable, calling the
                // model would spend money without a reliable record.
                ledger.release(estimate);
                return RunOutcome::Failed(i18n::t_args(
                    "Failed to record the budget reservation: {error}",
                    &[("error", error)],
                ));
            }
            let call_id = ctx.ids.new_id("call");
            if let Err(error) = ctx.bus.publish(
                &ctx.session_id,
                "plugin-loop",
                &DomainEvent::ModelCallIssued {
                    call_id: call_id.clone(),
                    run_id: ctx.run_id.clone(),
                    step_id: step_id.clone(),
                    model: model.status().model,
                    max_output_tokens,
                },
            ) {
                ledger.release(estimate);
                return RunOutcome::Failed(i18n::t_args(
                    "Failed to record the model call: {error}",
                    &[("error", error)],
                ));
            }

            let request = ModelRequest {
                // The gateway resolves "default" through the active provider
                // profile (including model mappings).
                model: "default".to_string(),
                system: assembled.system.clone(),
                messages: assembled.messages.clone(),
                tools: tools.clone(),
                max_output_tokens,
                call_id: call_id.clone(),
                run_id: ctx.run_id.clone(),
                stream: env_bool("PHI_STREAM", true),
                ui: ctx.ui_mode,
            };

            // The unified deadline applies to the model request too.
            let result = match tokio::time::timeout_at(
                tokio::time::Instant::from_std(ctx.deadline),
                model.invoke(request, ctx.cancel.clone()),
            )
            .await
            {
                Err(_) => {
                    ledger.release(estimate);
                    let _ = ctx.bus.publish(
                        &ctx.session_id,
                        "plugin-loop",
                        &DomainEvent::BudgetExceeded {
                            run_id: ctx.run_id.clone(),
                            dimension: "wall_time".to_string(),
                        },
                    );
                    return aborted(
                        ctx,
                        &i18n::t("Wall-clock limit exceeded"),
                        &i18n::t("Stopped: model request exceeded the wall-clock limit."),
                    );
                }
                Ok(result) => result,
            };

            match result {
                Ok(resp) => {
                    // Providers that omit usage must be settled at the
                    // reserved amount, not at zero.
                    let settled = if resp.estimated_usage {
                        estimate
                    } else {
                        resp.cost_usd
                    };
                    ledger.settle(estimate, settled);
                    if let Err(publish_error) = ctx.bus.publish(
                        &ctx.session_id,
                        "plugin-loop",
                        &DomainEvent::BudgetSettled {
                            run_id: ctx.run_id.clone(),
                            cost_usd: settled,
                            input_tokens: resp.input_tokens,
                            output_tokens: resp.output_tokens,
                            estimated: resp.estimated_usage,
                        },
                    ) {
                        return RunOutcome::Failed(i18n::t_args(
                            "Failed to record the budget settlement: {error}",
                            &[("error", publish_error)],
                        ));
                    }
                    if let Err(publish_error) = ctx.bus.publish(
                        &ctx.session_id,
                        "plugin-loop",
                        &DomainEvent::ModelCallReturned {
                            call_id,
                            run_id: ctx.run_id.clone(),
                            input_tokens: resp.input_tokens,
                            output_tokens: resp.output_tokens,
                            cost_usd: settled,
                            finish_reason: resp.finish_reason.clone(),
                        },
                    ) {
                        return RunOutcome::Failed(i18n::t_args(
                            "Failed to record the model response: {error}",
                            &[("error", publish_error)],
                        ));
                    }
                    set_status_progress(ctx, step_index, ledger.spent);
                    // Enforce the cost ceiling immediately after settlement;
                    // the estimate can be exceeded by actual usage.
                    if ledger.spent > ctx.max_cost_usd + 1e-9 {
                        let _ = ctx.bus.publish(
                            &ctx.session_id,
                            "plugin-loop",
                            &DomainEvent::BudgetExceeded {
                                run_id: ctx.run_id.clone(),
                                dimension: "cost".to_string(),
                            },
                        );
                        return aborted(
                            ctx,
                            &i18n::t("Cost limit exceeded"),
                            &i18n::t("Stopped: cost limit exceeded after settlement."),
                        );
                    }
                    if Instant::now() >= ctx.deadline {
                        let _ = ctx.bus.publish(
                            &ctx.session_id,
                            "plugin-loop",
                            &DomainEvent::BudgetExceeded {
                                run_id: ctx.run_id.clone(),
                                dimension: "wall_time".to_string(),
                            },
                        );
                        return aborted(
                            ctx,
                            &i18n::t("Wall-clock limit exceeded"),
                            &i18n::t(
                                "Stopped: model response arrived after the wall-clock deadline.",
                            ),
                        );
                    }
                    response = Some(resp);
                    break;
                }
                Err(error) => {
                    // Failed calls may still have cost; be conservative.
                    let charged = if error.message == "cancelled" {
                        0.0
                    } else {
                        estimate
                    };
                    ledger.settle(estimate, charged);
                    if let Err(publish_error) = ctx.bus.publish(
                        &ctx.session_id,
                        "plugin-loop",
                        &DomainEvent::BudgetSettled {
                            run_id: ctx.run_id.clone(),
                            cost_usd: charged,
                            input_tokens: 0,
                            output_tokens: 0,
                            estimated: true,
                        },
                    ) {
                        return RunOutcome::Failed(i18n::t_args(
                            "Failed to record the budget settlement: {error}",
                            &[("error", publish_error)],
                        ));
                    }
                    if let Err(publish_error) = ctx.bus.publish(
                        &ctx.session_id,
                        "plugin-loop",
                        &DomainEvent::ModelCallFailed {
                            call_id,
                            run_id: ctx.run_id.clone(),
                            error: error.message.clone(),
                        },
                    ) {
                        return RunOutcome::Failed(i18n::t_args(
                            "Failed to record the model failure: {error}",
                            &[("error", publish_error)],
                        ));
                    }
                    if ledger.spent > ctx.max_cost_usd + 1e-9 {
                        let _ = ctx.bus.publish(
                            &ctx.session_id,
                            "plugin-loop",
                            &DomainEvent::BudgetExceeded {
                                run_id: ctx.run_id.clone(),
                                dimension: "cost".to_string(),
                            },
                        );
                        return aborted(
                            ctx,
                            &i18n::t("Cost limit exceeded"),
                            &i18n::t("Stopped: cost limit exceeded after settlement."),
                        );
                    }
                    if error.message == "cancelled" || ctx.cancel.is_cancelled() {
                        return aborted(
                            ctx,
                            &i18n::t("Cancelled by user"),
                            &i18n::t(
                                "Stopped: model request was aborted; no tool side effects occurred.",
                            ),
                        );
                    }
                    if error.retryable && attempt == 1 {
                        let backoff = Duration::from_secs(1)
                            .min(ctx.deadline.saturating_duration_since(Instant::now()));
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {}
                            _ = ctx.cancel.cancelled() => {
                                return aborted(
                                    ctx,
                                    &i18n::t("Cancelled by user"),
                                    &i18n::t("Stopped: cancelled during retry backoff."),
                                );
                            }
                        }
                        continue;
                    }
                    return RunOutcome::Failed(error.message);
                }
            }
        }

        let Some(mut response) = response else {
            return RunOutcome::Failed(i18n::t("Model call returned no response"));
        };

        // Normalize tool ids so tool_use/tool_result pairs always match.
        for block in &mut response.blocks {
            if let ContentBlock::ToolUse { id, .. } = block {
                if id.trim().is_empty() {
                    *id = ctx.ids.new_id("call");
                }
            }
        }

        let tool_calls: Vec<(String, String, Value)> = response
            .blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.clone(), name.clone(), input.clone()))
                }
                _ => None,
            })
            .collect();

        if let Err(error) = session.append_message(
            &ctx.session_id,
            Some(&ctx.turn_id),
            Role::Assistant,
            response.blocks.clone(),
        ) {
            return RunOutcome::Failed(i18n::t_args(
                "Failed to save assistant message: {error}",
                &[("error", error)],
            ));
        }

        if tool_calls.is_empty() {
            if let Err(error) = ctx.bus.publish(
                &ctx.session_id,
                "plugin-loop",
                &DomainEvent::StepCompleted {
                    run_id: ctx.run_id.clone(),
                    step_id,
                },
            ) {
                return RunOutcome::Failed(i18n::t_args(
                    "Failed to record the step completion: {error}",
                    &[("error", error)],
                ));
            }
            return RunOutcome::Succeeded;
        }

        let mut result_blocks = Vec::new();
        let mut unknown_side_effect = false;
        let mut cancelled = false;
        let mut wall_exceeded = false;
        for (call_id, tool_id, input) in tool_calls {
            // Once cancellation or the wall deadline is observed, never start
            // another tool. Still emit a paired result so the history stays
            // valid for the provider.
            if cancelled || ctx.cancel.is_cancelled() {
                cancelled = true;
                result_blocks.push(ContentBlock::tool_result(
                    call_id,
                    i18n::t("Run cancelled; action was not executed."),
                    true,
                ));
                continue;
            }
            if wall_exceeded || Instant::now() >= ctx.deadline {
                wall_exceeded = true;
                result_blocks.push(ContentBlock::tool_result(
                    call_id,
                    i18n::t("Wall-clock limit exceeded; action was not executed."),
                    true,
                ));
                continue;
            }
            let outcome = execute_tool(
                ctx,
                &step_id,
                ToolCall {
                    id: call_id,
                    tool_id,
                    input,
                },
            )
            .await;
            if outcome.cancelled {
                cancelled = true;
            }
            if outcome.unknown_side_effect {
                unknown_side_effect = true;
            }
            if Instant::now() >= ctx.deadline {
                wall_exceeded = true;
            }
            result_blocks.push(outcome.block);
        }

        if let Err(error) = session.append_message(
            &ctx.session_id,
            Some(&ctx.turn_id),
            Role::User,
            result_blocks,
        ) {
            return RunOutcome::Failed(i18n::t_args(
                "Failed to save tool results: {error}",
                &[("error", error)],
            ));
        }

        if let Err(error) = ctx.bus.publish(
            &ctx.session_id,
            "plugin-loop",
            &DomainEvent::StepCompleted {
                run_id: ctx.run_id.clone(),
                step_id,
            },
        ) {
            return RunOutcome::Failed(i18n::t_args(
                "Failed to record the step completion: {error}",
                &[("error", error)],
            ));
        }

        if wall_exceeded {
            let _ = ctx.bus.publish(
                &ctx.session_id,
                "plugin-loop",
                &DomainEvent::BudgetExceeded {
                    run_id: ctx.run_id.clone(),
                    dimension: "wall_time".to_string(),
                },
            );
            return aborted(
                ctx,
                &i18n::t("Wall-clock limit exceeded"),
                &i18n::t(
                    "Stopped: wall-clock limit reached. A command that was still running may have produced partial side effects; verify the workspace.",
                ),
            );
        }

        if cancelled || ctx.cancel.is_cancelled() {
            let stop = if unknown_side_effect {
                i18n::t(
                    "Stopped: the command was terminated and may have produced partial side effects. Inspect and verify the workspace (no automatic retry).",
                )
            } else {
                i18n::t("Stopped: the in-flight tool call was aborted.")
            };
            return RunOutcome::Aborted {
                reason: i18n::t("Cancelled by user"),
                stop,
            };
        }
    }

    let _ = ctx.bus.publish(
        &ctx.session_id,
        "plugin-loop",
        &DomainEvent::BudgetExceeded {
            run_id: ctx.run_id.clone(),
            dimension: "steps".to_string(),
        },
    );
    RunOutcome::Aborted {
        reason: i18n::t("Step limit reached"),
        stop: i18n::t("Stopped: maximum steps reached; the task may be incomplete."),
    }
}

struct ToolExecOutcome {
    block: ContentBlock,
    cancelled: bool,
    unknown_side_effect: bool,
}

async fn execute_tool(ctx: &RunContext, step_id: &str, call: ToolCall) -> ToolExecOutcome {
    let digest = action_digest_for(ctx, &call);
    // The request record must be durable before anything else happens; if the
    // journal is unwritable we must not proceed to a side effect.
    if let Err(error) = ctx.bus.publish(
        &ctx.session_id,
        "plugin-loop",
        &DomainEvent::ToolInvokeRequested {
            invocation_id: call.id.clone(),
            run_id: ctx.run_id.clone(),
            step_id: step_id.to_string(),
            tool_id: call.tool_id.clone(),
            args_preview: args_preview(&call.tool_id, &call.input),
            action_digest: digest.clone(),
        },
    ) {
        return ToolExecOutcome {
            block: ContentBlock::tool_result(
                call.id.clone(),
                i18n::t_args(
                    "Action not executed: journal write failed ({error}).",
                    &[("error", error.to_string())],
                ),
                true,
            ),
            cancelled: false,
            unknown_side_effect: false,
        };
    }

    let policy_input = PolicyInput {
        tool_id: call.tool_id.clone(),
        args: call.input.clone(),
        workspace: ctx.workspace.clone(),
    };

    let Some(policy) = ctx.services.policy() else {
        let rationale = i18n::t("Policy component is unavailable; denied by default");
        let _ = decide(ctx, &call, "deny", &rationale);
        return deny_result(&call, &rationale);
    };
    let decision = policy.evaluate(&policy_input);

    match decision.effect {
        PolicyEffect::Deny => {
            let _ = decide(ctx, &call, "deny", &decision.rationale);
            return deny_result(&call, &decision.rationale);
        }
        PolicyEffect::Modify => {
            let rationale = i18n::t("modify is not supported in this version; denied");
            let _ = decide(ctx, &call, "deny", &rationale);
            return deny_result(&call, &rationale);
        }
        PolicyEffect::Ask => {
            if let Err(error) = decide(ctx, &call, "ask", &decision.rationale) {
                return tool_not_executed(
                    &call,
                    &i18n::t_args(
                        "Action not executed: journal write failed ({error}).",
                        &[("error", error.to_string())],
                    ),
                );
            }
            let Some(approver) = ctx.services.approver() else {
                let rationale = i18n::t("Approval component is unavailable; denied by default");
                let _ = decide(ctx, &call, "deny", &rationale);
                return deny_result(&call, &rationale);
            };
            let risk = if call.tool_id == "shell_exec" {
                i18n::t("Irreversible: code execution and permissions")
            } else {
                i18n::t("Reversible: data modification")
            };
            // Approval must not outlive the run's wall-clock deadline.
            let remaining_wall_secs = ctx
                .deadline
                .saturating_duration_since(Instant::now())
                .as_secs()
                .max(1);
            let spec = phi_contracts::ports::ApprovalSpec {
                session_id: ctx.session_id.clone(),
                run_id: ctx.run_id.clone(),
                invocation_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                args_preview: args_preview(&call.tool_id, &call.input),
                action_digest: digest.clone(),
                risk,
                expires_in_secs: env_u64("PHI_APPROVAL_TIMEOUT_SECS", 300).min(remaining_wall_secs),
            };
            let pending = match approver.request(spec) {
                Ok(pending) => pending,
                Err(error) => {
                    let rationale =
                        i18n::t_args("Failed to create approval: {error}", &[("error", error)]);
                    let _ = decide(ctx, &call, "deny", &rationale);
                    return deny_result(&call, &rationale);
                }
            };
            if let Err(error) = ctx.bus.publish(
                &ctx.session_id,
                "plugin-loop",
                &DomainEvent::RunWaitingApproval {
                    run_id: ctx.run_id.clone(),
                    approval_id: pending.id.clone(),
                },
            ) {
                let _ = approver.resolve(&pending.id, false, Some(i18n::t("Journal unavailable")));
                return tool_not_executed(
                    &call,
                    &i18n::t_args(
                        "Action not executed: journal write failed ({error}).",
                        &[("error", error.to_string())],
                    ),
                );
            }
            set_status(ctx, "WaitingApproval");
            let outcome = approver.wait(&pending.id, ctx.cancel.clone()).await;
            if outcome.status != "Approved" {
                let status = outcome.status.clone();
                let rationale = outcome.note.unwrap_or_else(|| {
                    i18n::t_args("Approval outcome: {status}", &[("status", status)])
                });
                let _ = decide(ctx, &call, "deny", &rationale);
                if ctx.cancel.is_cancelled() {
                    return ToolExecOutcome {
                        block: ContentBlock::tool_result(
                            call.id.clone(),
                            i18n::t("Run cancelled; action was not executed."),
                            true,
                        ),
                        cancelled: true,
                        unknown_side_effect: false,
                    };
                }
                return deny_result(&call, &rationale);
            }
            if let Err(error) = ctx.bus.publish(
                &ctx.session_id,
                "plugin-loop",
                &DomainEvent::RunResumed {
                    run_id: ctx.run_id.clone(),
                    approval_id: pending.id.clone(),
                },
            ) {
                return tool_not_executed(
                    &call,
                    &i18n::t_args(
                        "Action not executed: journal write failed ({error}).",
                        &[("error", error.to_string())],
                    ),
                );
            }
            set_status(ctx, "Running");
            // Approval is bound to the resolved resource identity and content,
            // not just the path string. Recompute and compare before running.
            let current = action_digest_for(ctx, &call);
            if current != pending.action_digest {
                let rationale =
                    i18n::t("The action changed after approval; the approval is no longer valid");
                let _ = decide(ctx, &call, "deny", &rationale);
                return deny_result(&call, &rationale);
            }
            if let Err(error) = decide(ctx, &call, "allow", &i18n::t("Human approval granted")) {
                return tool_not_executed(
                    &call,
                    &i18n::t_args(
                        "Action not executed: journal write failed ({error}).",
                        &[("error", error.to_string())],
                    ),
                );
            }
        }
        PolicyEffect::Allow => {
            if let Err(error) = decide(ctx, &call, "allow", &decision.rationale) {
                return tool_not_executed(
                    &call,
                    &i18n::t_args(
                        "Action not executed: journal write failed ({error}).",
                        &[("error", error.to_string())],
                    ),
                );
            }
        }
    }

    let Some(sandbox) = ctx.services.sandbox() else {
        let error = i18n::t(
            "Sandbox component is unavailable; execution denied (never run without isolation)",
        );
        publish_tool_failed(ctx, &call, &error);
        return ToolExecOutcome {
            block: ContentBlock::tool_result(call.id.clone(), error, true),
            cancelled: false,
            unknown_side_effect: false,
        };
    };
    let _ = sandbox; // The tool provider owns path resolution/exec via the same port.

    let Some(tool) = ctx.services.find_tool(&call.tool_id) else {
        let error = i18n::t_args(
            "No tool implementation: {tool_id}",
            &[("tool_id", call.tool_id.clone())],
        );
        publish_tool_failed(ctx, &call, &error);
        return ToolExecOutcome {
            block: ContentBlock::tool_result(call.id.clone(), error, true),
            cancelled: false,
            unknown_side_effect: false,
        };
    };

    if ctx.cancel.is_cancelled() {
        return ToolExecOutcome {
            block: ContentBlock::tool_result(
                call.id.clone(),
                i18n::t("Run cancelled; action was not executed."),
                true,
            ),
            cancelled: true,
            unknown_side_effect: false,
        };
    }

    if Instant::now() >= ctx.deadline {
        let error = i18n::t("Wall-clock limit exceeded; action was not executed.");
        publish_tool_failed(ctx, &call, &error);
        return tool_not_executed(&call, &error);
    }

    // The start record must be durable before the side effect begins.
    if let Err(error) = ctx.bus.publish(
        &ctx.session_id,
        "plugin-loop",
        &DomainEvent::ToolInvokeStarted {
            invocation_id: call.id.clone(),
            run_id: ctx.run_id.clone(),
        },
    ) {
        return tool_not_executed(
            &call,
            &i18n::t_args(
                "Action not executed: journal write failed ({error}).",
                &[("error", error.to_string())],
            ),
        );
    }

    let tool_ctx = ToolContext {
        workspace: ctx.workspace.clone(),
        session_id: ctx.session_id.clone(),
        run_id: ctx.run_id.clone(),
        deadline: Some(ctx.deadline),
    };
    let started = Instant::now();
    // Do not drop this future on cancellation: the execution layer (sandbox)
    // receives the token, kills the process tree and only then returns. Racing
    // it against the token here would abandon the termination confirmation.
    let result = tool.invoke(&call, &tool_ctx, ctx.cancel.clone()).await;

    match result {
        Ok(output) => {
            let duration_ms = started.elapsed().as_millis() as u64;
            let result_ref = ctx
                .blob
                .put(output.content.as_bytes())
                .unwrap_or_else(|_| "unavailable".to_string());
            // The side effect already happened. If its completion record
            // cannot be persisted, report that the audit trail is incomplete.
            if let Err(error) = ctx.bus.publish(
                &ctx.session_id,
                "plugin-loop",
                &DomainEvent::ToolInvokeCompleted {
                    invocation_id: call.id.clone(),
                    run_id: ctx.run_id.clone(),
                    result_ref,
                    is_error: output.is_error,
                    duration_ms,
                },
            ) {
                return ToolExecOutcome {
                    block: ContentBlock::tool_result(
                        call.id.clone(),
                        i18n::t_args(
                            "Action executed, but its audit record could not be written: {error}",
                            &[("error", error)],
                        ),
                        true,
                    ),
                    cancelled: false,
                    unknown_side_effect: call.tool_id == "shell_exec",
                };
            }
            ToolExecOutcome {
                block: ContentBlock::tool_result(
                    call.id.clone(),
                    truncate_chars(&output.content, 20_000),
                    output.is_error,
                ),
                cancelled: false,
                unknown_side_effect: false,
            }
        }
        Err(error) => {
            if error == "cancelled" {
                let evidence = i18n::t(
                    "The command was terminated and we cannot confirm whether side effects occurred; the run is awaiting reconciliation and will not retry automatically.",
                );
                let _ = ctx.bus.publish(
                    &ctx.session_id,
                    "plugin-loop",
                    &DomainEvent::ToolInvokeOutcomeUnknown {
                        invocation_id: call.id.clone(),
                        run_id: ctx.run_id.clone(),
                        evidence: evidence.clone(),
                    },
                );
                ToolExecOutcome {
                    block: ContentBlock::tool_result(call.id.clone(), evidence, true),
                    cancelled: true,
                    unknown_side_effect: call.tool_id == "shell_exec",
                }
            } else {
                publish_tool_failed(ctx, &call, &error);
                ToolExecOutcome {
                    block: ContentBlock::tool_result(call.id.clone(), error, true),
                    cancelled: false,
                    unknown_side_effect: false,
                }
            }
        }
    }
}

/// Compute the approval-bound action digest, including resolved resource
/// identity and content hash so a changed file, a swapped junction or a
/// different target invalidates an earlier approval.
fn action_digest_for(ctx: &RunContext, call: &ToolCall) -> String {
    let mut resources = Vec::new();
    if let Some(requested) = call.input.get("path").and_then(|value| value.as_str()) {
        let (resolved, kind, content_hash) = match ctx.services.sandbox() {
            Some(sandbox) => match sandbox.resolve_path(&ctx.workspace, requested) {
                Ok(path) => {
                    let (kind, content_hash) = resource_fingerprint(&path);
                    (path.to_string_lossy().to_string(), kind, content_hash)
                }
                Err(_) => ("<unresolved>".to_string(), "denied", None),
            },
            None => ("<no-sandbox>".to_string(), "unknown", None),
        };
        resources.push(ResourceIdentity {
            requested: requested.to_string(),
            resolved,
            kind,
            content_hash,
        });
    }
    action_digest(&call.tool_id, &call.input, &ctx.workspace, &resources)
}

fn decide(
    ctx: &RunContext,
    call: &ToolCall,
    decision: &str,
    rationale: &str,
) -> Result<(), String> {
    ctx.bus
        .publish(
            &ctx.session_id,
            "plugin-loop",
            &DomainEvent::ToolInvokeDecided {
                invocation_id: call.id.clone(),
                run_id: ctx.run_id.clone(),
                decision: decision.to_string(),
                rationale: rationale.to_string(),
            },
        )
        .map(|_| ())
        .map_err(|error| {
            i18n::t_args(
                "Failed to record the tool decision: {error}",
                &[("error", error.to_string())],
            )
        })
}

fn tool_not_executed(call: &ToolCall, reason: &str) -> ToolExecOutcome {
    ToolExecOutcome {
        block: ContentBlock::tool_result(call.id.clone(), reason.to_string(), true),
        cancelled: false,
        unknown_side_effect: false,
    }
}

fn publish_tool_failed(ctx: &RunContext, call: &ToolCall, error: &str) {
    let _ = ctx.bus.publish(
        &ctx.session_id,
        "plugin-loop",
        &DomainEvent::ToolInvokeFailed {
            invocation_id: call.id.clone(),
            run_id: ctx.run_id.clone(),
            error: error.to_string(),
        },
    );
}

fn deny_result(call: &ToolCall, rationale: &str) -> ToolExecOutcome {
    ToolExecOutcome {
        block: ContentBlock::tool_result(
            call.id.clone(),
            i18n::t_args(
                "Action denied: {rationale}",
                &[("rationale", rationale.to_string())],
            ),
            true,
        ),
        cancelled: false,
        unknown_side_effect: false,
    }
}

fn aborted(_ctx: &RunContext, reason: &str, stop: &str) -> RunOutcome {
    RunOutcome::Aborted {
        reason: reason.to_string(),
        stop: stop.to_string(),
    }
}

fn set_status(ctx: &RunContext, state: &str) {
    let mut status = ctx.status.lock().unwrap();
    status.state = state.to_string();
}

fn set_status_progress(ctx: &RunContext, steps: u64, cost: f64) {
    let mut status = ctx.status.lock().unwrap();
    status.steps = steps;
    status.cost_usd = cost;
    if status.state == "Scheduled" {
        status.state = "Running".to_string();
    }
}

fn fallback_context(history: Vec<ChatMessage>, workspace: &Path) -> AssembledContext {
    AssembledContext {
        system: format!(
            "You are phi-agent, a coding agent. Workspace: {}. Use tools to inspect and modify files; writes and shell commands require approval.",
            workspace.display()
        ),
        messages: history,
    }
}

fn estimate_chars(
    system: &str,
    messages: &[ChatMessage],
    tools: &[phi_contracts::ports::ToolDefinition],
) -> usize {
    let mut total = system.chars().count();
    for message in messages {
        for block in &message.blocks {
            total += match block {
                ContentBlock::Text { text } => text.chars().count(),
                ContentBlock::ToolUse { input, .. } => input.to_string().chars().count(),
                ContentBlock::ToolResult { content, .. } => content.chars().count(),
            };
        }
    }
    for tool in tools {
        total += tool.description.chars().count() + tool.input_schema.to_string().chars().count();
    }
    total
}

fn estimate_bytes(
    system: &str,
    messages: &[ChatMessage],
    tools: &[phi_contracts::ports::ToolDefinition],
) -> usize {
    let mut total = system.len() + 256;
    for message in messages {
        // Per-message structural overhead (role, delimiters).
        total += 32;
        for block in &message.blocks {
            total += match block {
                ContentBlock::Text { text } => text.len(),
                ContentBlock::ToolUse { input, .. } => input.to_string().len(),
                ContentBlock::ToolResult { content, .. } => content.len(),
            };
        }
    }
    for tool in tools {
        total += 64 + tool.description.len() + tool.input_schema.to_string().len();
    }
    total
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

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            !(value == "0" || value == "false" || value == "off" || value == "no")
        })
        .unwrap_or(default)
}
