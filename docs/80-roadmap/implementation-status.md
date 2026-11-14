# 功能实现现状

> 状态：🌿 · 最后更新：2026-11-14
> 上游：`80-roadmap/evolution.md`（交付边界）、`README.md`（对用户的承诺）
> 判定口径：以仓库里的代码和自动化测试为准。设计文档说「应该怎样」，本文件说「现在做到哪」。

---

## 0. 一句话

MVP 验收线（桌面端完成一次真实编码任务：读取仓库 → 修改文件 → 运行验证 → 展示结果，并覆盖人工审批、预算上限、取消、崩溃恢复）**已实现且有自动化测试**。缺口集中在两处：

1. **桌面设置面**：Provider 管理、语言切换、会话管理这些后端已具备的能力，桌面前端尚未接全。
2. **v1 路线图项**：记忆、多端 attach/换端、可安装插件、多 Agent 尚未开工。

---

## 1. 图例

| 图例 | 含义 |
|---|---|
| ✅ | 已实现，有单测或集成测试覆盖 |
| 🟡 | 部分实现，缺口明确 |
| ⛔ | 未实现（路线图项或设计预留） |

---

## 2. 总览

| 能力域 | 状态 | 实现位置 | 验证 |
|---|---|---|---|
| Agent 循环 | ✅ | `phi-plugins/src/loop_plugin.rs` | 单测 + headless 集成测试 |
| 工具集（文件 / Shell / 工作台） | ✅ | `phi-plugins/src/tools.rs`、`sandbox.rs` | `sandbox`、`headless` 测试 |
| 策略与人工审批 | ✅ | `phi-plugins/src/policy.rs`、`approver.rs` | `policy` 单测、审批集成测试 |
| 预算三重上限 | ✅ | `loop_plugin.rs`（`Ledger`） | `step_limit_aborts_the_run` |
| 取消与崩溃恢复 | ✅ | `loop_plugin.rs`、`session.rs`、`node.rs` | 重启/中断相关测试 |
| 会话、事件日志与投影 | ✅ | `phi-plugins/src/session.rs`、`phi-adapters` | `sessions_survive_a_restart` |
| 模型网关（三种协议 + 流式） | ✅ | `model_gateway.rs`、`model_protocol.rs` | 协议单测 |
| Provider 档案 | ✅ | `provider_store.rs` | `provider_store` 单测 |
| 工作台视图（v1 协议） | ✅ | `phi-contracts/src/ui.rs`、`src/workbench/` | `ui` 单测、`workbench_run_updates_the_session_view` |
| 无头 CLI | ✅ | `phi-headless/src/main.rs` | `phi-headless/tests/headless.rs` |
| 内核与内置插件生命周期 | ✅ | `phi-kernel`、`phi-node` | `lifecycle` 单测 |
| 桌面 UI | 🟡 | `src/App.tsx` | `tsc` + 构建；设置面缺口见 §4 |
| i18n | 🟡 | `phi-i18n`、`src/i18n.ts` | `core_messages_have_translations` |
| OS Keychain | 🟡 | `phi-contracts/src/provider.rs` | 如实报错，未接系统钥匙串 |
| 可安装插件 / 插件 ABI | ⛔ | — | v1 |
| 记忆 | ⛔ | — | v1 |
| 多端 attach / 换端 | ⛔ | — | v1 / 后续 |
| 多 Agent、辅助模型审批 | ⛔ | 端口已预留 | 后续 |
| 本地模型 | ⛔ | — | 路线图 |
| VM 级沙箱 | ⛔ | — | 以审批为边界，README 已说明 |

---

## 3. MVP 线内已实现

### 3.1 Agent 循环与预算 ✅

- 单次用户输入 → 调度 Run → 循环调用模型 → 执行工具 → 回填结果 → 结束。
- 默认上限：**20 步 / $0.50 / 600 秒**，可用 `PHI_MAX_STEPS`、`PHI_MAX_COST_USD`、`PHI_MAX_WALL_SECS` 覆盖。
- 每次模型调用**先预留预算再发起**，调用结束后按实际用量结算；预留失败即中止，硬上限不依赖调用后扣费。
- 模型调用、审批等待、工具执行共享同一个墙钟 deadline；超时按中止处理。
- 取消令牌在步间、模型调用和工具执行中都会被检查。
- 上下文折叠保留 `tool_use` / `tool_result` 配对，丢弃孤立结果，避免中断后把残缺对话发给模型。

### 3.2 工具集 ✅

| 工具 | 行为 | 备注 |
|---|---|---|
| `fs_list` | 列出工作区内目录 | 默认 `.` |
| `fs_read` | 读取文件，支持 `offset` / `limit` | 默认 400 行 |
| `fs_write` | 创建或覆盖文件，可建父目录 | 旧内容先备份；审批后才执行 |
| `shell_exec` | 在工作区执行命令 | 默认 120 秒、上限 300 秒，输出截断 20k 字符 |
| `ui_update` | 替换工作台视图树 | 仅 workbench 模式暴露 |

- 文件路径经沙箱解析，`..`、绝对路径、盘符越界一律拒绝。
- 工具结果以内容寻址 blob 保存，会话投影按需还原。

### 3.3 策略与人工审批 ✅

默认策略（无匹配规则 = 拒绝，最小权限）：

| 动作 | 决策 |
|---|---|
| 工作区内只读 | 允许 |
| 工作区内写入 | 询问（人工审批） |
| Shell 命令 | 询问；命中危险模式（`rm -rf /`、`mkfs`、`shutdown`、`dd if=` 等）直接拒绝 |
| `ui_update` | 允许（纯声明式数据，不执行代码） |
| 未知工具 / 越界路径 | 拒绝 |

审批语义：

- 审批请求绑定**动作摘要**（工具 + 规范化参数 + 工作区 + 资源指纹）；执行前重新计算摘要，参数一变即失效。
- 审批有有效期；超时按拒绝处理，取消 Run 会使审批作废。
- 批准 / 拒绝均可附备注，结果写入事件日志。
- 每个 Run 同时最多一个待审批动作，桌面 UI 内联展示并可直接批准 / 拒绝。

### 3.4 取消与崩溃恢复 ✅

- 取消是协作式的：令牌会传入模型调用、工具执行和审批等待。
- 已经产生副作用的取消被标记为「结果未知」，不会假装成功。
- 进程重启时，未完成的 Run 被投影为 `AwaitingReconciliation`（需要人工核对），**不自动重试**。
- 未记录到工具结果的 `tool_use` 会被修复/标记，避免会话带着悬空调用继续跑。

### 3.5 会话与持久化 ✅

- 会话创建、列表、按 id 读取；启动时恢复上次会话。
- 事件以 JSONL journal 落盘，内容寻址 blob 存大块内容；会话视图由事件投影得到。
- 跨进程可读：另一个进程通过投影读取已完成 Run 的状态。
- 重启后会话、消息、Run 状态、工作台视图都还在。

缺口：

- 无会话重命名 / 删除 / 搜索 / 导出。
- 标题默认是「New session」，不会根据首条消息自动生成。
- 无分页；会话很多时一次性返回。

### 3.6 模型网关 ✅

- 支持三种线上协议：`anthropic_messages`、`openai_chat`、`openai_responses`，均有 SSE 流式解析。
- 流式增量只发给外壳显示，最终助手消息才进入事件日志。
- HTTP 错误分类；模型列表拉取兼容 `/v1`、完整端点与自定义 base URL。
- 无可用 Provider 时使用内置 mock 模型，可离线走完整的工具与审批流程。
- Provider 档案持久化在数据目录，支持自定义 headers、认证模式、定价和模型列表。
- 凭据以引用保存：`env:NAME`、`literal:VALUE`、`keychain:NAME`；读取接口只返回脱敏提示。
- `keychain:` 引用尚未接系统钥匙串，会**如实报错**而不是静默失败。

### 3.7 工作台视图 ✅

- 协议版本 1：`{ version, title?, root: { kind, props?, children? } }`。
- 节点种类白名单（15 种）：布局、文本、代码、表格、键值、图表、表单、按钮、徽章、进度等。
- 校验边界：最大深度 16、最多 800 节点、200k 字节上限；动作只允许「发消息」或「打开 http(s) URL」。
- 视图作为 `UiViewUpdated` 事件入库并投影到会话视图，重启后仍在。
- 前端用 Solid 组件注册表渲染，未知组件显示占位提示，不执行模型生成的代码。
- 表单提交、按钮点击会把指令作为新消息发回同一会话；URL 通过系统打开。
- 工作台模式按 Run 记录；普通对话不暴露 `ui_update`，也不注入协议提示。

### 3.8 无头 CLI ✅

- 命令：`run`、`sessions list/show`、`status`、`approve`、`cancel`、`bootstrap`、`providers list/show/add/update/remove/use/test/models`。
- 退出码：`0` 成功、`2` 用法/配置错误、`3` 中止、`4` 失败、`5` 待核对。
- 事件与模型增量是 stdout 上的 JSONL；日志与摘要走 stderr。
- 写模式独占数据目录锁；只读命令不加锁，直接读日志投影。
- 跨进程 `approve` / `cancel` 通过 `<data-dir>/control/` 控制文件投递。

### 3.9 内核与内置插件 ✅

- 微内核：事件总线、生命周期、注册表、机制端口；核心无领域词汇。
- 必需槽位缺失 → 进入 Maintenance；插件停用 → 贡献移除。
- 当前只有**内置 Rust 插件**（模型网关、工具、策略、审批、会话投影、沙箱），没有外部插件加载。

### 3.10 桌面 UI 🟡

已实现：

- 会话侧栏（列表、切换、新建）、工作区路径设置。
- 模型状态徽章；未配置时可直接填 API Key。
- 对话转录：文本、工具调用、工具结果（可展开）。
- 审批卡片（风险、参数、批准 / 拒绝）、Run 状态条（步数、费用、tokens、取消）。
- 失败 / 中止 / 待核对横幅与原因。
- 工作台开关与工作台面板；窄屏自动上下堆叠。

缺口：

- **没有 Provider 设置面**：后端 `list_providers` / `save_provider` / `remove_provider` / `set_active_provider` / `test_provider` / `fetch_models` 均已具备，桌面 UI 只用了旧的单 Key 入口。
- 没有语言切换入口（`set_language` 已有，UI 未调用）。
- 没有会话重命名 / 删除 / 搜索 / 导出。
- 没有独立的历史运行详情页；运行信息只在当前会话内展示。

### 3.11 i18n 🟡

- English 为消息 id，中文只存在于 `phi-i18n` 与 `src/i18n.ts`。
- 无头模式支持 `--lang`；桌面 UI 从 bootstrap 读取语言。
- 尚无桌面内的语言切换控件。

---

## 4. 缺口与优先级

| 优先级 | 缺口 | 现状 | 影响 |
|---|---|---|---|
| P0 | Provider 设置 UI | 后端与 CLI 完成，桌面未接 | 用户无法在桌面管理多 Provider、测试连接、拉取模型 |
| P1 | 语言切换入口 | 后端命令可用 | 用户无法在应用内切换语言 |
| P1 | 会话管理 | 未实现 | 会话多起来后无法整理 |
| P1 | OS Keychain | 引用类型存在，适配器未接 | 生产环境密钥只能落盘或走环境变量 |
| P2 | 记忆 | 未实现 | 跨会话没有事实记忆（v1） |
| P2 | 可安装插件 / ABI | 未实现 | 生态扩展（v1） |
| P2 | 多端 attach / 换端 | 未实现 | 多设备续跑（v1 / 后续） |
| P3 | 多 Agent / 辅助模型审批 | 端口已预留 | 后续迭代 |
| P3 | 本地模型 | 未实现 | 完全离线（路线图） |
| P3 | VM 级沙箱 | 未实现 | Shell 以用户权限运行，审批是边界（README 已明示） |
| P3 | Task 一等实体 | 事件带 `task_id`，UI 无任务视图 | ADR-0008 的可选项 |
| P3 | 移动端 | CI 能构建 Android | 无体验验证，不作为 v1 门槛 |

---

## 5. 与 README 承诺的对照

| README 承诺 | 现状 |
|---|---|
| 本地优先，数据与密钥留在本机 | ✅ 会话、日志、Provider 档案都在数据目录 |
| 无 Key 也能离线体验完整流程 | ✅ 内置 mock 模型 |
| 写入与命令先审批，审批绑定具体动作 | ✅ 动作摘要绑定，改参即失效 |
| 文件工具限制在工作区 | ✅ 沙箱路径校验 |
| 步数 / 费用 / 时间三重上限 | ✅ 调用前预留 |
| 取消真实有效，结果未知会标注 | ✅ |
| 崩溃后标记待核对，不自动重试 | ✅ |
| 工作台而非聊天窗口 | ✅ 视图树 + Solid 渲染 |
| 多设备续跑 | ⛔ 与 README 路线图一致（Planned） |

---

## 6. 验证入口

```bash
# Rust：62 个测试全量
cd src-tauri && cargo test --workspace

# 前端类型检查与构建
bunx tsc --noEmit && bun run build
```

CI 校验项：Rust 格式、前端类型检查与构建、Windows 全量测试、Linux 无桌面壳测试。

关键测试：

- `step_limit_aborts_the_run`、`write_approval_allow_creates_the_file`、`write_approval_deny_leaves_the_workspace_untouched`
- `sessions_and_journal_survive_a_restart`、`status_reads_a_finished_run_from_a_different_process`
- `workbench_run_updates_the_session_view`
- `policy::tests::*`、`provider_store::tests::*`、`ui::tests::*`

---

## 7. 维护约定

- 功能行为变化时同步更新本文件；以代码和测试为准，不复制设计文档。
- 新增补充材料（功能现状、实现说明、设计补充）一律在 `docs/` 下起**新文件**，并在 `docs/README.md` 登记；不要把它们堆进 README 或 AGENT.md。
- README 只保留用户可感知的能力与承诺。
