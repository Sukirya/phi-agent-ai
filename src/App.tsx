import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type ModelStatus = { provider: string; model: string; configured: boolean };
type SessionSummary = {
  id: string;
  title: string;
  createdAt: string;
  updatedAt: string;
  messageCount: number;
  lastRunState: string | null;
};
type Block =
  | { type: "text"; text: string }
  | { type: "tool_use"; id: string; name: string; input: unknown }
  | { type: "tool_result"; tool_use_id: string; content: string; is_error: boolean };
type Message = {
  id: string;
  turnId: string | null;
  role: "user" | "assistant";
  ts: string;
  blocks: Block[];
};
type Run = {
  id: string;
  taskId: string;
  turnId: string;
  state: string;
  steps: number;
  maxSteps: number;
  costUsd: number;
  maxCostUsd: number;
  maxWallSecs: number;
  inputTokens: number;
  outputTokens: number;
  pendingApprovalId: string | null;
  error: string | null;
  stopConfirmation: string | null;
  startedAt: string | null;
  updatedAt: string;
};
type SessionView = {
  id: string;
  ownerNodeId: string;
  title: string;
  createdAt: string;
  updatedAt: string;
  messages: Message[];
  runs: Run[];
};
type PendingApproval = {
  id: string;
  sessionId: string;
  runId: string;
  invocationId: string;
  toolId: string;
  argsPreview: unknown;
  actionDigest: string;
  risk: string;
  requestedAt: string;
  expiresAt: string;
};
type SessionDetail = { session: SessionView; pendingApprovals: PendingApproval[] };
type Bootstrap = {
  nodeId: string;
  mode: string;
  modeReason: string;
  model: ModelStatus;
  workspace: string | null;
  sessions: SessionSummary[];
  lastSessionId: string | null;
  maxSteps: number;
  maxCostUsd: number;
  maxWallSecs: number;
};

function errorText(error: unknown): string {
  return typeof error === "string" ? error : String(error);
}

function BlockView(props: { block: Block }) {
  switch (props.block.type) {
    case "text":
      return <div class="text">{props.block.text}</div>;
    case "tool_use":
      return (
        <details class="tool">
          <summary>🔧 调用 {props.block.name}</summary>
          <pre>{JSON.stringify(props.block.input, null, 2)}</pre>
        </details>
      );
    case "tool_result":
      return (
        <details classList={{ tool: true, error: props.block.is_error }}>
          <summary>{props.block.is_error ? "✗ 工具结果" : "✓ 工具结果"}</summary>
          <pre>{props.block.content}</pre>
        </details>
      );
  }
}

function App() {
  const [boot, setBoot] = createSignal<Bootstrap | null>(null);
  const [detail, setDetail] = createSignal<SessionDetail | null>(null);
  const [currentId, setCurrentId] = createSignal<string | null>(null);
  const [input, setInput] = createSignal("");
  const [workspaceInput, setWorkspaceInput] = createSignal("");
  const [apiKey, setApiKey] = createSignal("");
  const [error, setError] = createSignal("");
  let transcriptRef: HTMLElement | undefined;
  let refreshTimer: number | undefined;

  async function refresh() {
    const bootstrap = await invoke<Bootstrap>("get_bootstrap");
    setBoot(bootstrap);
    if (!workspaceInput() && bootstrap.workspace) {
      setWorkspaceInput(bootstrap.workspace);
    }
    const id = currentId() ?? bootstrap.lastSessionId;
    if (id) {
      setCurrentId(id);
      const next = await invoke<SessionDetail>("get_session", { sessionId: id });
      setDetail(next);
    }
  }

  function scheduleRefresh() {
    if (refreshTimer) window.clearTimeout(refreshTimer);
    refreshTimer = window.setTimeout(() => {
      refresh().catch((e) => setError(errorText(e)));
    }, 120);
  }

  onMount(() => {
    refresh().catch((e) => setError(errorText(e)));
    let unlisten: (() => void) | undefined;
    listen("phi://event", () => scheduleRefresh()).then((fn) => {
      unlisten = fn;
    });
    onCleanup(() => unlisten?.());
  });

  createEffect(() => {
    const messages = detail()?.session.messages.length ?? 0;
    const approvals = detail()?.pendingApprovals.length ?? 0;
    void messages;
    void approvals;
    if (transcriptRef) {
      queueMicrotask(() => {
        if (transcriptRef) transcriptRef.scrollTop = transcriptRef.scrollHeight;
      });
    }
  });

  async function send() {
    const text = input().trim();
    const id = currentId();
    if (!text || !id) return;
    setError("");
    setInput("");
    try {
      await invoke("send_message", { sessionId: id, text });
    } catch (e) {
      setError(errorText(e));
      setInput(text);
    }
    scheduleRefresh();
  }

  async function resolveApproval(approvalId: string, approved: boolean) {
    setError("");
    try {
      await invoke("approve", { approvalId, approved, note: null });
    } catch (e) {
      setError(errorText(e));
    }
    scheduleRefresh();
  }

  async function cancelRun(runId: string) {
    setError("");
    try {
      await invoke("cancel_run", { runId });
    } catch (e) {
      setError(errorText(e));
    }
    scheduleRefresh();
  }

  async function newSession() {
    setError("");
    try {
      const summary = await invoke<SessionSummary>("create_session", { title: null });
      setCurrentId(summary.id);
      setDetail(await invoke<SessionDetail>("get_session", { sessionId: summary.id }));
      await refresh();
    } catch (e) {
      setError(errorText(e));
    }
  }

  async function selectSession(id: string) {
    setCurrentId(id);
    try {
      setDetail(await invoke<SessionDetail>("get_session", { sessionId: id }));
    } catch (e) {
      setError(errorText(e));
    }
  }

  async function saveWorkspace() {
    setError("");
    try {
      const saved = await invoke<string>("set_workspace", { path: workspaceInput() });
      setWorkspaceInput(saved);
      await refresh();
    } catch (e) {
      setError(errorText(e));
    }
  }

  async function saveApiKey() {
    setError("");
    try {
      const status = await invoke<ModelStatus>("set_api_key", { key: apiKey() });
      const current = boot();
      if (current) setBoot({ ...current, model: status });
      setApiKey("");
    } catch (e) {
      setError(errorText(e));
    }
  }

  const currentRun = () => {
    const runs = detail()?.session.runs ?? [];
    return runs.length > 0 ? runs[runs.length - 1] : null;
  };
  const isRunning = () => {
    const state = currentRun()?.state;
    return state === "Scheduled" || state === "Running" || state === "WaitingApproval";
  };
  const modeOk = () => boot()?.mode === "Normal";

  return (
    <div class="app">
      <aside class="sidebar">
        <div class="brand">phi-agent</div>
        <button class="new-chat" onClick={newSession}>
          + 新会话
        </button>
        <div class="session-list">
          <For each={boot()?.sessions ?? []}>
            {(session) => (
              <button
                classList={{ session: true, active: session.id === currentId() }}
                onClick={() => selectSession(session.id)}
              >
                <span class="session-title">{session.title}</span>
                <span class="session-meta">
                  {new Date(session.updatedAt).toLocaleString()} · {session.messageCount} 条
                </span>
              </button>
            )}
          </For>
        </div>
        <div class="sidebar-foot">
          <Show when={boot()}>
            {(b) => (
              <>
                <div>节点 {b().nodeId.slice(0, 14)}…</div>
                <div>模式 {b().mode}</div>
              </>
            )}
          </Show>
        </div>
      </aside>

      <main class="main">
        <header class="topbar">
          <div class="workspace">
            <label>工作区</label>
            <input
              value={workspaceInput()}
              onInput={(e) => setWorkspaceInput(e.currentTarget.value)}
              placeholder="工作区绝对路径"
            />
            <button onClick={saveWorkspace}>保存</button>
          </div>
          <div class="model">
            <Show
              when={boot()?.model.configured}
              fallback={
                <div class="key-row">
                  <input
                    type="password"
                    value={apiKey()}
                    onInput={(e) => setApiKey(e.currentTarget.value)}
                    placeholder="ANTHROPIC_API_KEY（仅保存在内存）"
                  />
                  <button onClick={saveApiKey}>设置</button>
                </div>
              }
            >
              <span class="badge ok">{boot()?.model.provider} / {boot()?.model.model}</span>
            </Show>
            <span classList={{ badge: true, bad: !modeOk() }}>{boot()?.mode}</span>
          </div>
        </header>

        <Show when={!modeOk()}>
          <div class="banner error">
            {boot()?.modeReason || "节点未处于正常模式，发送消息已被禁用"}
          </div>
        </Show>
        <Show when={error()}>
          <div class="banner error">{error()}</div>
        </Show>

        <section class="transcript" ref={(el) => (transcriptRef = el)}>
          <Show when={detail()} fallback={<div class="empty">加载中…</div>}>
            {(d) => (
              <>
                <Show when={d().session.messages.length === 0}>
                  <div class="empty">
                    <p>向编码 Agent 描述一个任务，例如：</p>
                    <p class="hint">“先看看这个仓库的结构，然后修掉那个失败的测试。”</p>
                    <Show when={!boot()?.model.configured}>
                      <p class="hint">
                        当前没有配置模型密钥，运行会使用 Mock 模型（只演示工具回路）。可以在右上角设置密钥。
                      </p>
                    </Show>
                  </div>
                </Show>
                <For each={d().session.messages}>
                  {(message) => (
                    <div class={`msg ${message.role}`}>
                      <div class="msg-role">{message.role === "user" ? "你" : "Agent"}</div>
                      <div class="msg-body">
                        <For each={message.blocks}>{(block) => <BlockView block={block} />}</For>
                      </div>
                    </div>
                  )}
                </For>

                <For each={d().pendingApprovals}>
                  {(approval) => (
                    <div class="approval">
                      <div class="approval-title">⚠ 需要审批：{approval.toolId}</div>
                      <div class="approval-risk">风险：{approval.risk}</div>
                      <pre>{JSON.stringify(approval.argsPreview, null, 2)}</pre>
                      <div class="approval-actions">
                        <button class="primary" onClick={() => resolveApproval(approval.id, true)}>
                          批准
                        </button>
                        <button onClick={() => resolveApproval(approval.id, false)}>拒绝</button>
                      </div>
                      <div class="approval-foot">动作摘要 {approval.actionDigest.slice(0, 16)}… · 到期 {approval.expiresAt}</div>
                    </div>
                  )}
                </For>
              </>
            )}
          </Show>
        </section>

        <Show when={currentRun()}>
          {(run) => (
            <div class="runbar">
              <span classList={{ state: true, active: isRunning() }}>{run().state}</span>
              <span>
                步数 {run().steps}/{run().maxSteps}
              </span>
              <span>
                ${run().costUsd.toFixed(4)} / ${run().maxCostUsd.toFixed(2)}
              </span>
              <span>
                tokens {run().inputTokens + run().outputTokens}
              </span>
              <Show when={isRunning()}>
                <button class="danger" onClick={() => cancelRun(run().id)}>
                  取消
                </button>
              </Show>
            </div>
          )}
        </Show>

        <Show
          when={currentRun()?.state === "AwaitingReconciliation" || currentRun()?.state === "Failed" || currentRun()?.state === "Aborted"}
        >
          <div class="banner warn">
            <div>{currentRun()?.error}</div>
            <div>{currentRun()?.stopConfirmation}</div>
          </div>
        </Show>

        <footer class="composer">
          <textarea
            value={input()}
            onInput={(e) => setInput(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send();
              }
            }}
            placeholder="输入任务…（Enter 发送，Shift+Enter 换行）"
            disabled={!modeOk() || isRunning()}
            rows={3}
          />
          <div class="composer-actions">
            <span class="hint">预算：{boot()?.maxSteps ?? 20} 步 / ${boot()?.maxCostUsd ?? 0.5}</span>
            <button class="primary" onClick={send} disabled={!modeOk() || isRunning() || !input().trim()}>
              发送
            </button>
          </div>
        </footer>
      </main>
    </div>
  );
}

export default App;
