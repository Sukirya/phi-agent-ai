import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { lang, setLang, t, tArgs } from "./i18n";
import { ViewRenderer } from "./workbench/ViewRenderer";
import type { ViewAction, WorkbenchView } from "./workbench/types";
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
  workbenchMode: boolean;
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
  uiView: WorkbenchView | null;
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
type SendResult = { runId: string };
type ModelStreamEvent = {
  runId: string;
  callId: string;
  kind: string;
  text?: string;
};
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
  language: string;
};

function errorText(error: unknown): string {
  return typeof error === "string" ? error : String(error);
}

const STATE_LABELS: Record<string, string> = {
  Scheduled: "Scheduled",
  Running: "Running",
  WaitingApproval: "Waiting for approval",
  Succeeded: "Succeeded",
  Failed: "Failed",
  Aborted: "Aborted",
  AwaitingReconciliation: "Needs review",
  Normal: "Normal",
  Degraded: "Degraded",
  Maintenance: "Maintenance",
};

function stateLabel(state: string): string {
  return t(STATE_LABELS[state] ?? state);
}

function BlockView(props: { block: Block }) {
  switch (props.block.type) {
    case "text":
      return <div class="text">{props.block.text}</div>;
    case "tool_use":
      return (
        <details class="tool">
          <summary>{tArgs("🔧 Call {name}", { name: props.block.name })}</summary>
          <pre>{JSON.stringify(props.block.input, null, 2)}</pre>
        </details>
      );
    case "tool_result":
      return (
        <details classList={{ tool: true, error: props.block.is_error }}>
          <summary>
            {props.block.is_error ? t("✗ Tool result") : t("✓ Tool result")}
          </summary>
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
  const [workbenchMode, setWorkbenchMode] = createSignal(false);
  const [error, setError] = createSignal("");
  const [streamText, setStreamText] = createSignal("");
  const [streamRunId, setStreamRunId] = createSignal<string | null>(null);
  let transcriptRef: HTMLElement | undefined;
  let refreshTimer: number | undefined;

  async function refresh() {
    const bootstrap = await invoke<Bootstrap>("get_bootstrap");
    setLang(bootstrap.language);
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
    let unlistenEvent: (() => void) | undefined;
    let unlistenStream: (() => void) | undefined;
    listen<{ payload?: { type?: string } }>("phi://event", (event) => {
      const type = event.payload?.payload?.type;
      if (
        type === "RunSucceeded" ||
        type === "RunFailed" ||
        type === "RunAborted"
      ) {
        setStreamText("");
        setStreamRunId(null);
      }
      scheduleRefresh();
    }).then((fn) => {
      unlistenEvent = fn;
    });
    listen<ModelStreamEvent>("phi://model-stream", (event) => {
      const payload = event.payload;
      if (payload.kind === "text") {
        const runId = streamRunId();
        if (runId && payload.runId !== runId) return;
        if (!runId) setStreamRunId(payload.runId);
        setStreamText((current) => current + (payload.text ?? ""));
      } else if (payload.kind === "done") {
        const runId = streamRunId();
        if (runId && payload.runId !== runId) return;
        window.setTimeout(() => {
          setStreamText("");
          setStreamRunId(null);
        }, 250);
      }
    }).then((fn) => {
      unlistenStream = fn;
    });
    onCleanup(() => {
      unlistenEvent?.();
      unlistenStream?.();
    });
  });

  createEffect(() => {
    const messages = detail()?.session.messages.length ?? 0;
    const approvals = detail()?.pendingApprovals.length ?? 0;
    const streaming = streamText().length;
    void messages;
    void approvals;
    void streaming;
    if (transcriptRef) {
      queueMicrotask(() => {
        if (transcriptRef) transcriptRef.scrollTop = transcriptRef.scrollHeight;
      });
    }
  });

  async function sendText(text: string, fromComposer: boolean) {
    const id = currentId();
    if (!text || !id) return;
    setError("");
    if (fromComposer) setInput("");
    setStreamText("");
    setStreamRunId(null);
    try {
      const result = await invoke<SendResult>("send_message", {
        sessionId: id,
        text,
        workbench: workbenchMode(),
      });
      setStreamRunId(result.runId);
    } catch (e) {
      setError(errorText(e));
      if (fromComposer) setInput(text);
    }
    scheduleRefresh();
  }

  function send() {
    const text = input().trim();
    if (text) void sendText(text, true);
  }

  async function handleViewAction(action: ViewAction) {
    if (action.kind === "message") {
      await sendText(action.text, false);
      return;
    }
    try {
      await openUrl(action.url);
    } catch (e) {
      setError(errorText(e));
    }
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
    setStreamText("");
    setStreamRunId(null);
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
    setStreamText("");
    setStreamRunId(null);
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
  const uiView = () => detail()?.session.uiView ?? null;
  const showWorkbench = () => !!uiView() || workbenchMode();

  return (
    <div class="app">
      <aside class="sidebar">
        <div class="brand">phi-agent</div>
        <button class="new-chat" onClick={newSession}>
          {t("+ New session")}
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
                  {tArgs("{date} · {count} messages", {
                    date: new Date(session.updatedAt).toLocaleString(
                      lang() === "zh-CN" ? "zh-CN" : "en-US",
                    ),
                    count: session.messageCount,
                  })}
                </span>
              </button>
            )}
          </For>
        </div>
        <div class="sidebar-foot">
          <Show when={boot()}>
            {(b) => (
              <>
                <div>{tArgs("Node {id}…", { id: b().nodeId.slice(0, 14) })}</div>
                <div>{tArgs("Mode {mode}", { mode: stateLabel(b().mode) })}</div>
              </>
            )}
          </Show>
        </div>
      </aside>

      <main class="main">
        <header class="topbar">
          <div class="workspace">
            <label>{t("Workspace")}</label>
            <input
              value={workspaceInput()}
              onInput={(e) => setWorkspaceInput(e.currentTarget.value)}
              placeholder={t("Absolute workspace path")}
            />
            <button onClick={saveWorkspace}>{t("Save")}</button>
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
                    placeholder={t("ANTHROPIC_API_KEY (stored locally on this device)")}
                  />
                  <button onClick={saveApiKey}>{t("Set")}</button>
                </div>
              }
            >
              <span class="badge ok">{boot()?.model.provider} / {boot()?.model.model}</span>
            </Show>
            <span classList={{ badge: true, bad: !modeOk() }}>{stateLabel(boot()?.mode ?? "")}</span>
          </div>
        </header>

        <Show when={!modeOk()}>
          <div class="banner error">
            {boot()?.modeReason || t("The node is not in normal mode; sending is disabled")}
          </div>
        </Show>
        <Show when={error()}>
          <div class="banner error">{error()}</div>
        </Show>

        <div classList={{ "main-body": true, split: showWorkbench() }}>
          <section class="conversation">
            <section class="transcript" ref={(el) => (transcriptRef = el)}>
              <Show when={detail()} fallback={<div class="empty">{t("Loading…")}</div>}>
                {(d) => (
                  <>
                    <Show when={d().session.messages.length === 0}>
                      <div class="empty">
                        <p>{t("Describe a task for the coding agent, for example:")}</p>
                        <p class="hint">
                          {t("“Look at the repository structure, then fix the failing test.”")}
                        </p>
                        <Show when={!boot()?.model.configured}>
                          <p class="hint">
                            {t(
                              "No model key is configured, so runs use the mock model (it demonstrates the tool loop). Set a key in the top right.",
                            )}
                          </p>
                        </Show>
                      </div>
                    </Show>
                    <For each={d().session.messages}>
                      {(message) => (
                        <div class={`msg ${message.role}`}>
                          <div class="msg-role">
                            {message.role === "user" ? t("You") : t("Agent")}
                          </div>
                          <div class="msg-body">
                            <For each={message.blocks}>
                              {(block) => <BlockView block={block} />}
                            </For>
                          </div>
                        </div>
                      )}
                    </For>

                    <Show when={streamText()}>
                      <div class="msg assistant streaming">
                        <div class="msg-role">{t("Agent")}</div>
                        <div class="msg-body">
                          <div class="text">{streamText()}</div>
                        </div>
                      </div>
                    </Show>

                    <For each={d().pendingApprovals}>
                      {(approval) => (
                        <div class="approval">
                          <div class="approval-title">
                            {tArgs("⚠ Approval required: {tool}", {
                              tool: approval.toolId,
                            })}
                          </div>
                          <div class="approval-risk">
                            {tArgs("Risk: {risk}", { risk: approval.risk })}
                          </div>
                          <pre>{JSON.stringify(approval.argsPreview, null, 2)}</pre>
                          <div class="approval-actions">
                            <button class="primary" onClick={() => resolveApproval(approval.id, true)}>
                              {t("Approve")}
                            </button>
                            <button onClick={() => resolveApproval(approval.id, false)}>
                              {t("Reject")}
                            </button>
                          </div>
                          <div class="approval-foot">
                            {tArgs("Action digest {digest}… · expires {expires}", {
                              digest: approval.actionDigest.slice(0, 16),
                              expires: approval.expiresAt,
                            })}
                          </div>
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
                  <span classList={{ state: true, active: isRunning() }}>{stateLabel(run().state)}</span>
                  <span>
                    {tArgs("Steps {steps}/{max}", {
                      steps: run().steps,
                      max: run().maxSteps,
                    })}
                  </span>
                  <span>
                    ${run().costUsd.toFixed(4)} / ${run().maxCostUsd.toFixed(2)}
                  </span>
                  <span>
                    tokens {run().inputTokens + run().outputTokens}
                  </span>
                  <Show when={isRunning()}>
                    <button class="danger" onClick={() => cancelRun(run().id)}>
                      {t("Cancel")}
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
                placeholder={t("Describe a task… (Enter to send, Shift+Enter for a newline)")}
                disabled={!modeOk() || isRunning()}
                rows={3}
              />
              <div class="composer-actions">
                <label
                  class="ui-toggle"
                  title={t(
                    "Workbench mode: the agent also shapes the task workbench next to the conversation.",
                  )}
                >
                  <input
                    type="checkbox"
                    checked={workbenchMode()}
                    onChange={(e) => setWorkbenchMode(e.currentTarget.checked)}
                  />
                  {t("Workbench")}
                </label>
                <span class="hint">
                  {tArgs("Budget: {steps} steps / ${cost}", {
                    steps: boot()?.maxSteps ?? 20,
                    cost: boot()?.maxCostUsd ?? 0.5,
                  })}
                </span>
                <button class="primary" onClick={send} disabled={!modeOk() || isRunning() || !input().trim()}>
                  {t("Send")}
                </button>
              </div>
            </footer>
          </section>

          <Show when={showWorkbench()}>
            <aside class="workbench-pane">
              <header class="workbench-head">
                <span class="workbench-title">
                  {uiView()?.title || t("Workbench")}
                </span>
                <Show when={uiView()}>
                  {(view) => (
                    <span class="badge">
                      {tArgs("View v{version}", { version: view().version })}
                    </span>
                  )}
                </Show>
              </header>
              <div class="workbench-body">
                <Show
                  when={uiView()}
                  fallback={
                    <div class="workbench-empty">
                      {t("The agent will lay out the task here when a visual overview helps.")}
                    </div>
                  }
                >
                  {(view) => (
                    <ViewRenderer view={view()} onAction={handleViewAction} />
                  )}
                </Show>
              </div>
            </aside>
          </Show>
        </div>
      </main>
    </div>
  );
}

export default App;
