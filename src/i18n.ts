// Frontend internationalization.
//
// English message ids are the source; the Chinese catalog is the only place
// that contains translated text. The Rust node selects the language and
// exposes it in Bootstrap, so both shells agree on the setting.

export type Lang = "en" | "zh-CN";

import { createSignal } from "solid-js";

const [current, setCurrent] = createSignal<Lang>("en");

export function setLang(value: string | null | undefined) {
  setCurrent(value === "zh-CN" ? "zh-CN" : "en");
}

export function lang(): Lang {
  return current();
}

const ZH: Record<string, string> = {
  "🔧 Call {name}": "🔧 调用 {name}",
  "✗ Tool result": "✗ 工具结果",
  "✓ Tool result": "✓ 工具结果",
  "+ New session": "+ 新会话",
  "Node {id}…": "节点 {id}…",
  "Mode {mode}": "模式 {mode}",
  "Workspace": "工作区",
  "Absolute workspace path": "工作区绝对路径",
  "Save": "保存",
  "ANTHROPIC_API_KEY (stored locally on this device)": "ANTHROPIC_API_KEY（保存在本机）",
  "Set": "设置",
  "The node is not in normal mode; sending is disabled": "节点未处于正常模式，发送消息已被禁用",
  "Loading…": "加载中…",
  "Describe a task for the coding agent, for example:": "向编码 Agent 描述一个任务，例如：",
  "“Look at the repository structure, then fix the failing test.”": "“先看看这个仓库的结构，然后修掉那个失败的测试。”",
  "No model key is configured, so runs use the mock model (it demonstrates the tool loop). Set a key in the top right.": "当前没有配置模型密钥，运行会使用 Mock 模型（只演示工具回路）。可以在右上角设置密钥。",
  "You": "你",
  "Agent": "Agent",
  "{date} · {count} messages": "{date} · {count} 条",
  "⚠ Approval required: {tool}": "⚠ 需要审批：{tool}",
  "Risk: {risk}": "风险：{risk}",
  "Approve": "批准",
  "Reject": "拒绝",
  "Action digest {digest}… · expires {expires}": "动作摘要 {digest}… · 到期 {expires}",
  "Steps {steps}/{max}": "步数 {steps}/{max}",
  "Cancel": "取消",
  "Describe a task… (Enter to send, Shift+Enter for a newline)": "输入任务…（Enter 发送，Shift+Enter 换行）",
  "Budget: {steps} steps / ${cost}": "预算：{steps} 步 / ${cost}",
  "Send": "发送",
  "Workbench": "工作台",
  "Workbench mode: the agent also shapes the task workbench next to the conversation.": "工作台模式：Agent 会同时在对话旁布置任务工作台。",
  "View v{version}": "视图 v{version}",
  "Unsupported view component: {kind}": "不支持的视图组件：{kind}",
  "The agent will lay out the task here when a visual overview helps.": "当可视化的总览有帮助时，Agent 会在这里布置任务。",
  "Scheduled": "已排程",
  "Running": "运行中",
  "Waiting for approval": "等待审批",
  "Succeeded": "已完成",
  "Failed": "失败",
  "Aborted": "已中止",
  "Needs review": "待核对",
  "Normal": "正常",
  "Degraded": "降级",
  "Maintenance": "维护",
};

export function t(key: string): string {
  if (current() === "en") return key;
  return ZH[key] ?? key;
}

export function tArgs(
  key: string,
  args: Record<string, string | number>,
): string {
  let text = t(key);
  for (const [name, value] of Object.entries(args)) {
    text = text.split(`{${name}}`).join(String(value));
  }
  return text;
}
