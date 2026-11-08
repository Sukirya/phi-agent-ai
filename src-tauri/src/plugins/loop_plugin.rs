use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::contracts::ports::{
    AssembledContext, ModelRequest, PolicyEffect, PolicyInput, RunStatus, RuntimePort,
    SessionPort, StartRunRequest, ToolCall, ToolContext,
};
use crate::contracts::util::{
    action_digest, args_preview, resource_fingerprint, truncate_chars, ResourceIdentity,
};
use crate::contracts::{SLOT_RUNTIME_SKELETON};
use crate::domain::events::DomainEvent;
use crate::domain::message::{ChatMessage, ContentBlock, Role};
use crate::kernel::event_bus::EventBus;
use crate::kernel::lifecycle::{Plugin, PluginContext};
use crate::kernel::ports::{BlobPort, IdPort, LogPort};
use crate::kernel::registry::Contribution;

use crate::contracts::services::Services;

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
        let session = self
            .services
            .session()
            .ok_or_else(|| "会话存储不可用（session.store 缺失）".to_string())?;
        if let Some(active) = session.active_run(&req.session_id) {
            return Err(format!("该会话仍有运行中的任务: {active}"));
        }
        if !req.workspace.is_dir() {
            return Err("工作区不存在或不是目录".to_string());
        }

        let run_id = self.ids.new_id("run");
        let task_id = self.ids.new_id("task");
        let max_steps = env_u64("PHI_MAX_STEPS", 20);
        let max_cost_usd = env_f64("PHI_MAX_COST_USD", 0.50);
        let max_wall_secs = env_u64("PHI_MAX_WALL_SECS", 600);

        self.bus.publish(
            &req.session_id,
            "plugin-loop",
            &DomainEvent::TaskCreated {
                task_id: task_id.clone(),
                title: "隐式任务".to_string(),
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
            log: self.log.clone(),
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
        };
        tokio::spawn(run_loop(ctx));
        Ok(run_id)
    }

    fn cancel(&self, run_id: &str, reason: &str) -> bool {
        let running = self.running.lock().unwrap();
        if let Some(control) = running.get(run_id) {
            self.log.info(&format!(
                "cancel requested for {run_id}: {reason}"
            ));
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
    log: Arc<dyn LogPort>,
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
}

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
        return RunOutcome::Failed("会话存储不可用".to_string());
    };
    let Some(model) = ctx.services.model() else {
        return RunOutcome::Failed("模型网关不可用".to_string());
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
    let tools = ctx.services.tool_definitions();
    let max_output_tokens = env_u64("PHI_MAX_OUTPUT_TOKENS", 4096) as u32;

    for step_index in 1..=ctx.max_steps {
        if ctx.cancel.is_cancelled() {
            return aborted(ctx, "用户取消", "已停止：进行中的请求已中止。");
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
            return aborted(ctx, "超过墙钟上限", "已停止：运行超过时间上限。");
        }

        let history = session.messages(&ctx.session_id);
        let assembled = match ctx.services.context() {
            Some(assembler) => assembler.assemble(history, &ctx.workspace),
            None => fallback_context(history, &ctx.workspace),
        };

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
            return RunOutcome::Failed(format!("journal write failed for step: {error}"));
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
                return aborted(ctx, "超过墙钟上限", "已停止：运行超过时间上限。");
            }
            let input_chars =
                estimate_chars(&assembled.system, &assembled.messages, &tools);
            let input_bytes =
                estimate_bytes(&assembled.system, &assembled.messages, &tools);
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
                return aborted(ctx, "预算超限（成本），调用前预留失败", "已停止：预算不足以发起下一次模型调用。");
            }
            if let Err(error) = ctx.bus.publish(
                &ctx.session_id,
                "plugin-loop",
                &DomainEvent::BudgetReserved {
                    run_id: ctx.run_id.clone(),
                    dimension: "cost".to_string(),
                    amount: estimate,
                    note: "模型调用前预留".to_string(),
                },
            ) {
                // If the reservation cannot be made durable, calling the
                // model would spend money without a reliable record.
                ledger.release(estimate);
                return RunOutcome::Failed(format!(
                    "journal write failed for budget reservation: {error}"
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
                return RunOutcome::Failed(format!(
                    "journal write failed for model call: {error}"
                ));
            }

            let request = ModelRequest {
                model: model.status().model,
                system: assembled.system.clone(),
                messages: assembled.messages.clone(),
                tools: tools.clone(),
                max_output_tokens,
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
                        "超过墙钟上限",
                        "Stopped: model request exceeded the wall-clock limit.",
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
                        return RunOutcome::Failed(format!(
                            "journal write failed for budget settlement: {publish_error}"
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
                        return RunOutcome::Failed(format!(
                            "journal write failed for model response: {publish_error}"
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
                            "超过成本上限",
                            "Stopped: cost limit exceeded after settlement.",
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
                            "超过墙钟上限",
                            "Stopped: model response arrived after the wall-clock deadline.",
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
                        return RunOutcome::Failed(format!(
                            "journal write failed for budget settlement: {publish_error}"
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
                        return RunOutcome::Failed(format!(
                            "journal write failed for model failure: {publish_error}"
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
                            "超过成本上限",
                            "Stopped: cost limit exceeded after settlement.",
                        );
                    }
                    if error.message == "cancelled" || ctx.cancel.is_cancelled() {
                        return aborted(
                            ctx,
                            "用户取消",
                            "已停止：模型请求已中止，未产生工具副作用。",
                        );
                    }
                    if error.retryable && attempt == 1 {
                        let backoff = Duration::from_secs(1).min(
                            ctx.deadline.saturating_duration_since(Instant::now()),
                        );
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {}
                            _ = ctx.cancel.cancelled() => {
                                return aborted(ctx, "用户取消", "已停止：重试等待期间取消。");
                            }
                        }
                        continue;
                    }
                    return RunOutcome::Failed(error.message);
                }
            }
        }

        let Some(mut response) = response else {
            return RunOutcome::Failed("模型调用没有得到响应".to_string());
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
            return RunOutcome::Failed(format!("保存助手消息失败: {error}"));
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
                return RunOutcome::Failed(format!(
                    "journal write failed for step completion: {error}"
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
                    "Run cancelled; action was not executed.",
                    true,
                ));
                continue;
            }
            if wall_exceeded || Instant::now() >= ctx.deadline {
                wall_exceeded = true;
                result_blocks.push(ContentBlock::tool_result(
                    call_id,
                    "Wall-clock limit exceeded; action was not executed.",
                    true,
                ));
                continue;
            }
            let outcome = execute_tool(
                ctx,
                &session,
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
            return RunOutcome::Failed(format!("保存工具结果失败: {error}"));
        }

        if let Err(error) = ctx.bus.publish(
            &ctx.session_id,
            "plugin-loop",
            &DomainEvent::StepCompleted {
                run_id: ctx.run_id.clone(),
                step_id,
            },
        ) {
            return RunOutcome::Failed(format!(
                "journal write failed for step completion: {error}"
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
                "超过墙钟上限",
                "Stopped: wall-clock limit reached. A command that was still running may have produced partial side effects; verify the workspace.",
            );
        }

        if cancelled || ctx.cancel.is_cancelled() {
            let stop = if unknown_side_effect {
                "已停止：命令被终止，但可能已产生部分副作用。请检查工作区并核对（不会自动重试）。"
            } else {
                "已停止：进行中的工具调用已中止。"
            };
            return RunOutcome::Aborted {
                reason: "用户取消".to_string(),
                stop: stop.to_string(),
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
        reason: "达到步数上限".to_string(),
        stop: "已停止：达到最大步数，任务可能未完成。".to_string(),
    }
}

struct ToolExecOutcome {
    block: ContentBlock,
    cancelled: bool,
    unknown_side_effect: bool,
}

async fn execute_tool(
    ctx: &RunContext,
    session: &Arc<dyn SessionPort>,
    step_id: &str,
    call: ToolCall,
) -> ToolExecOutcome {
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
            args_preview: args_preview(&call.input),
            action_digest: digest.clone(),
        },
    ) {
        return ToolExecOutcome {
            block: ContentBlock::tool_result(
                call.id.clone(),
                format!("Action not executed: journal write failed ({error})."),
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
        let rationale = "策略组件不可用，按默认拒绝处理";
        let _ = decide(ctx, &call, "deny", rationale);
        return deny_result(&call, rationale);
    };
    let decision = policy.evaluate(&policy_input);

    match decision.effect {
        PolicyEffect::Deny => {
            let _ = decide(ctx, &call, "deny", &decision.rationale);
            return deny_result(&call, &decision.rationale);
        }
        PolicyEffect::Modify => {
            let rationale = "当前版本不支持 modify，按拒绝处理";
            let _ = decide(ctx, &call, "deny", rationale);
            return deny_result(&call, rationale);
        }
        PolicyEffect::Ask => {
            if let Err(error) = decide(ctx, &call, "ask", &decision.rationale) {
                return tool_not_executed(&call, &format!(
                    "Action not executed: journal write failed ({error})."
                ));
            }
            let Some(approver) = ctx.services.approver() else {
                let rationale = "审批组件不可用，按默认拒绝处理";
                let _ = decide(ctx, &call, "deny", rationale);
                return deny_result(&call, rationale);
            };
            let risk = if call.tool_id == "shell_exec" {
                "不可逆：代码执行与权限"
            } else {
                "可逆：数据修改"
            };
            // Approval must not outlive the run's wall-clock deadline.
            let remaining_wall_secs = ctx
                .deadline
                .saturating_duration_since(Instant::now())
                .as_secs()
                .max(1);
            let spec = crate::contracts::ports::ApprovalSpec {
                session_id: ctx.session_id.clone(),
                run_id: ctx.run_id.clone(),
                invocation_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                args_preview: args_preview(&call.input),
                action_digest: digest.clone(),
                risk: risk.to_string(),
                expires_in_secs: env_u64("PHI_APPROVAL_TIMEOUT_SECS", 300)
                    .min(remaining_wall_secs),
            };
            let pending = match approver.request(spec) {
                Ok(pending) => pending,
                Err(error) => {
                    let rationale = format!("创建审批失败: {error}");
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
                let _ = approver.resolve(&pending.id, false, Some("journal unavailable".to_string()));
                return tool_not_executed(
                    &call,
                    &format!("Action not executed: journal write failed ({error})."),
                );
            }
            set_status(ctx, "WaitingApproval");
            let outcome = approver.wait(&pending.id, ctx.cancel.clone()).await;
            if outcome.status != "Approved" {
                let rationale = outcome
                    .note
                    .unwrap_or_else(|| format!("审批结果: {}", outcome.status));
                let _ = decide(ctx, &call, "deny", &rationale);
                if ctx.cancel.is_cancelled() {
                    return ToolExecOutcome {
                        block: ContentBlock::tool_result(
                            call.id.clone(),
                            "Run cancelled; action was not executed.",
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
                    &format!("Action not executed: journal write failed ({error})."),
                );
            }
            set_status(ctx, "Running");
            // Approval is bound to the resolved resource identity and content,
            // not just the path string. Recompute and compare before running.
            let current = action_digest_for(ctx, &call);
            if current != pending.action_digest {
                let rationale = "动作在审批后发生变化，审批失效";
                let _ = decide(ctx, &call, "deny", rationale);
                return deny_result(&call, rationale);
            }
            if let Err(error) = decide(ctx, &call, "allow", "人工审批通过") {
                return tool_not_executed(
                    &call,
                    &format!("Action not executed: journal write failed ({error})."),
                );
            }
        }
        PolicyEffect::Allow => {
            if let Err(error) = decide(ctx, &call, "allow", &decision.rationale) {
                return tool_not_executed(
                    &call,
                    &format!("Action not executed: journal write failed ({error})."),
                );
            }
        }
    }

    let Some(sandbox) = ctx.services.sandbox() else {
        let error = "沙箱组件不可用，拒绝执行（不会无隔离运行）";
        publish_tool_failed(ctx, &call, error);
        return ToolExecOutcome {
            block: ContentBlock::tool_result(call.id.clone(), error, true),
            cancelled: false,
            unknown_side_effect: false,
        };
    };
    let _ = sandbox; // The tool provider owns path resolution/exec via the same port.

    let Some(tool) = ctx.services.find_tool(&call.tool_id) else {
        let error = format!("没有工具实现: {}", call.tool_id);
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
                "Run cancelled; action was not executed.",
                true,
            ),
            cancelled: true,
            unknown_side_effect: false,
        };
    }

    if Instant::now() >= ctx.deadline {
        let error = "Wall-clock limit exceeded; action was not executed.";
        publish_tool_failed(ctx, &call, error);
        return tool_not_executed(&call, error);
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
            &format!("Action not executed: journal write failed ({error})."),
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
                        format!(
                            "Action executed, but its audit record could not be written: {error}"
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
                let evidence =
                    "命令被终止，无法确认是否已产生副作用；运行进入待核对，不会自动重试。";
                let _ = ctx.bus.publish(
                    &ctx.session_id,
                    "plugin-loop",
                    &DomainEvent::ToolInvokeOutcomeUnknown {
                        invocation_id: call.id.clone(),
                        run_id: ctx.run_id.clone(),
                        evidence: evidence.to_string(),
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
        .map_err(|error| format!("journal write failed for tool decision: {error}"))
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
            format!("动作被拒绝: {rationale}"),
            true,
        ),
        cancelled: false,
        unknown_side_effect: false,
    }
}

fn aborted(ctx: &RunContext, reason: &str, stop: &str) -> RunOutcome {
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

fn estimate_chars(system: &str, messages: &[ChatMessage], tools: &[crate::contracts::ports::ToolDefinition]) -> usize {
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

fn estimate_bytes(system: &str, messages: &[ChatMessage], tools: &[crate::contracts::ports::ToolDefinition]) -> usize {
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
