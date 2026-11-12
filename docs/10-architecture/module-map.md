# 模块地图与依赖规则

> 状态：🌿 · 最后更新：2026-11-08
> 关联：`10-architecture/overview.md`（决策与分层）、`00-overview/principles.md`（P8/P10）

本文回答三个问题：**系统由哪些模块组成？谁依赖谁？删掉一个会怎样？**

---

## 1. 模块总图

```
┌─────────────────────────────── 外壳层 Shell ───────────────────────────────┐
│  shell-tui   shell-desktop   shell-mobile   shell-web-console   shell-headless │
└───────────────────────────────────┬────────────────────────────────────────┘
                                    │ 只用：命令契约 + 事件流契约
┌───────────────────────────────────▼────────────────────────────────────────┐
│                            内核 Kernel（不可移除，极小）                       │
│  kernel-eventbus   kernel-registry   kernel-lifecycle                        │
│  kernel-ports(接口)  kernel-primitives(id/clock/log/config)                   │
└───────────────────────────────────┬────────────────────────────────────────┘
                                    │ 注册到扩展点 / 订阅事件
┌───────────────────────────────────▼──────── 插件层 Plugins ──────────────────┐
│                                                                              │
│  【运行时】           【能力】              【治理】          【协同】           │
│  plugin-loop          plugin-tools         plugin-policy    plugin-sync       │
│  plugin-orchestrator  plugin-skills        plugin-approver  plugin-transport  │
│  plugin-context       plugin-mcp                          ────────────────    │
│  plugin-memory        plugin-sandbox      【UI 扩展】        【数据】            │
│  plugin-model-gw     ────────────────      plugin-ui-ext    plugin-store        │
│  plugin-session      plugin-model-provider(anthropic/openai/local)              │
└───────────────────────────────────┬────────────────────────────────────────┘
                                    │ 实现端口（依赖倒置）
┌───────────────────────────────────▼───────── 适配器层 Adapters ──────────────┐
│  adapter-sqlite │ adapter-blob-fs │ adapter-ws │ adapter-http │ adapter-os-keychain │
│  adapter-js-engine │ adapter-process-sandbox │ adapter-container-sandbox           │
└──────────────────────────────────────────────────────────────────────────────┘
```

> **注意**：插件层分两类——
>
> - **发行基线**（`plugin-loop`、`plugin-session`、`plugin-policy`、`plugin-approver`、`plugin-sandbox`、`plugin-model-gw`、`plugin-store`、`plugin-context`）：官方发行**必装**，但**仍是普通插件**——实现可替换，无特权代码路径。缺失 → 进入 `Maintenance`/`Degraded`（见 §2.1）。
> - **可选插件**（工具、技能、记忆、编排、MCP、同步、UI 扩展…）：随意增删，P10 的「移除后照常」主要对它们断言。
>
> 这不是「有的插件高人一等」，而是**承认现实**：删掉模型网关后不可能还照常调用模型。详见 ADR-0009。

---

## 2. 内核模块（不可移除）

| 模块 | 职责 | 为什么必须在核心里 |
|---|---|---|
| `kernel-eventbus` | 发布/订阅事件，承载一切跨模块通信 | 是解耦的地基（P3） |
| `kernel-registry` | 类型化的注册表：扩展点、贡献、能力、服务 | 「一切皆插件」的机制（P1/P6） |
| `kernel-lifecycle` | 插件的发现→解析→激活→停用→卸载，以及**运行模式**判定 | 可增删的前提（P10） |
| `kernel-ports` | **机制性**端口接口定义（存储/事件日志、传输、时钟、ID、日志、配置） | 依赖倒置点（P2/P8） |
| `kernel-primitives` | ID、时钟、结构化日志、配置 | 谁都需要的极小工具 |

> **领域端口不在内核**：`ModelPort`、`MemoryPort`、`SandboxPort`、`ToolExecPort` 等由**所属领域的契约包**定义，内核不认识它们。只有机制性端口留在内核（总线要落盘，事件日志端口必须离得足够近）。

> **内核 API 预算**：以上五个模块的**公开接口**总表面积应被显式限制。新增内核公开 API 需要 ADR。这是防止「内核悄悄长大」的闸门（P1）。

### 2.1 必需槽位与运行模式（ADR-0009）

官方发行以配置声明**必需槽位**——必须被某个实现占据的**能力契约**（不是具体插件）：

| 必需槽位 | 契约 | 官方默认实现 |
|---|---|---|
| `runtime.skeleton` | Run/Step/检查点/预算记账 | `plugin-loop` |
| `session.store` | 会话/轮次/运行状态与事件产出 | `plugin-session` |
| `persistence.journal` | 事件日志的落盘与读取 | `plugin-store`（经机制性存储端口） |
| `policy.evaluate` | 动作判定 `allow/deny/ask/modify` | `plugin-policy` |
| `sandbox.execute` | 隔离执行 | `plugin-sandbox` |
| `approval.decide` | `ask` 类动作的裁决 | `plugin-approver` |
| `model.invoke` | 模型调用 | `plugin-model-gw` |

**运行模式**（替代「能启动/不能启动」的二元判断）：

| 模式 | 触发 | 行为 |
|---|---|---|
| `Normal` | 必需槽位齐备 | 全功能 |
| `Degraded` | **运行中**丢失安全组件（策略/沙箱/审批） | 禁止发起**新的**副作用动作；只读与既有任务可见 |
| `Maintenance` | **启动时**必需槽位缺失 | 仅诊断、安装插件、修复配置。**不拒绝进程启动**——否则用户没有修复入口 |

---

## 3. 插件模块

> 标记 **〔基线〕** 的是发行基线（必装、实现可换）；未标记的是可选插件。

### 3.1 运行时线

| 模块 | 职责 | 依赖端口 | 移除后果 |
|---|---|---|---|
| `plugin-loop` 〔基线〕 | 提供可插拔的行动循环策略（ReAct/plan-execute/…）与 **Run/Step/检查点/预算记账** | Model, Journal | 无运行时骨架 → `Maintenance`（必需槽位 `runtime.skeleton`） |
| `plugin-orchestrator` | 多 Agent 协作编排（supervisor/group-chat/pipeline） | Loop, Session | 只能单 Agent（v1 目标态） |
| `plugin-context` 〔基线〕 | 上下文构筑（分层组装、压缩） | Memory, Model | 上下文退化为原始堆叠 |
| `plugin-session` 〔基线〕 | 会话/轮次/运行的状态管理与事件产出 | Storage, Journal | 无会话模型 → `Maintenance`（必需槽位 `session.store`） |

### 3.2 能力线

| 模块 | 职责 | 依赖端口 | 移除后果 |
|---|---|---|---|
| `plugin-tools` | 工具注册与调用 | Sandbox, Policy | 无工具（只能对话） |
| `plugin-skills` | 技能的发现、加载、注入 | Storage | 无技能 |
| `plugin-mcp` | 接入 MCP 工具发现层 | Transport | 无外部 MCP 工具 |
| `plugin-sandbox` 〔基线〕 | 隔离执行工具/代码（含自带的 CLI 子进程） | Process/Container/Engine | **必需槽位 `sandbox.execute` 缺失 → `Maintenance`；运行中丢失 → `Degraded`，有副作用动作一律拒绝**（不是「无隔离跑」） |
| `plugin-model-gw` 〔基线〕 | 模型路由、缓存、用量、预算（**调用前预留额度**） | Model providers | **必需槽位 `model.invoke` 缺失 → `Maintenance`** |
| `plugin-model-provider-*` | 具体模型方适配（Anthropic/OpenAI/本地） | — | 少一种模型来源 |
| `plugin-memory` | 记忆存取（add/search + 变更操作） | Storage, Search | 无长期记忆（合法的空态） |

### 3.3 治理线

| 模块 | 职责 | 依赖端口 | 移除后果 |
|---|---|---|---|
| `plugin-policy` 〔基线〕 | 动作判定 allow/deny/ask/modify；**层内排序、跨层取交集** | Registry | **必需槽位 `policy.evaluate` 缺失 → `Maintenance`；运行中丢失 → `Degraded`，有副作用动作一律拒绝**（不是「默认允许或默认拒绝」的二选一） |
| `plugin-approver` 〔基线〕 | 审批者：人 / 辅助模型 | Model | **必需槽位 `approval.decide` 缺失 → 有副作用动作一律拒绝**；审批超时即 `Expired`→拒绝 |

### 3.4 协同线

| 模块 | 职责 | 依赖端口 | 移除后果 |
|---|---|---|---|
| `plugin-transport` 〔基线〕 | 本地/跨机的统一传输 | WS/HTTP/IPC | 只能是单机进程内 |
| `plugin-sync` | 事件/会话的跨节点复制、attach、换端（**以 Session 为单位**，ADR-0011） | Transport, Journal | 无协同（纯单机） |
| `plugin-store` 〔基线〕 | 本地持久化与命名空间；**事件日志落盘** | SQLite/Blob | **必需槽位 `persistence.journal` 缺失 → `Maintenance`**；不得用「写成功」的 Null 顶上 |

### 3.5 UI 线

| 模块 | 职责 | 移除后果 |
|---|---|---|
| `plugin-ui-ext` | UI 扩展点：面板、命令、视图 | 无第三方 UI 扩展 |
| `shell-*` | 具体外壳（默认提供） | 少一种前端形态 |

> **取舍说明**：上表把「模块」切到较细粒度，是为了让 P10 的可移除性有明确的验收单元。实现时若干小模块可**打包成一个制品**发布，但**接口边界必须保持**——打包是分发细节，不是耦合许可。

---

## 4. 依赖规则（可进 CI 检查）

**规则 R1 · 单向依赖**：`shell → kernel`、`plugin → kernel + ports`、`adapter → ports`。反向依赖一律失败。

**规则 R2 · 插件之间不直接引用**：`plugin-A` **不得** `import plugin-B`。要协作，通过**事件**或**注册表查询**。这条是 P10 的关键——没有它，删掉 B 会让 A 崩。

```
  ✅ 允许：   tool-plugin  ──发布事件 ToolInvoked──▶ EventBus ──订阅──▶ audit-plugin
  ❌ 禁止：   tool-plugin  ──use──▶ memory-plugin   （删掉 memory 就崩）
  ✅ 允许：   loop-plugin  ──registry.find(Approver)──▶ （拿不到就降级）
```

**规则 R3 · 端口必须有 Null 实现**：每个端口至少一个空实现，用于「模块缺席时降级启动」与测试。

**规则 R4 · 无环**：模块依赖图必须是 DAG。CI 用图检查。

**规则 R5 · 领域不依赖平台**：插件里的领域逻辑不得出现文件系统、socket、UI、数据库驱动等平台符号。

---

## 5. 可移除性验收（P10 的操作化）

P10 对**两类模块**有两种断言（ADR-0009）：

```
【可选模块】
[diff 测试]  移除 M 的注册 →
             ① 进程能否启动？                  （不能则 P10 失败）
             ② 不依赖 M 的冒烟用例是否全过？     （不过则 P10 失败）
             ③ 依赖 M 的功能是否「明确不可用」或按契约安全降级？

【基线 / 必需槽位】
[diff 测试]  移除该槽位的实现 →
             ① 系统是否进入 Maintenance（而非静默降级为放行）？
             ② 运行中移除安全组件 → 是否立即进入 Degraded 并拒绝新的副作用？

【Null 实现的诚实性（单测）】
             ① 只读端口 Null → 返回空集（合法）
             ② 写端口 Null → 必须报错/拒绝，**不得返回成功**（否则数据静默丢失）
```

建议维护一份 `smoke/` 冒烟矩阵：每个模块 × 启用/禁用两态。MVP 期至少覆盖「禁用每一个**可选**插件后能启动」+「移除每个**必需槽位**后进入 `Maintenance`」。

---

## 6. 技术映射（已定 / 推荐 / 待定）

> 铁律提醒：以下**不是**架构的一部分，是**实现选型**。状态见「状态」列。

| 层 | 选型 | 状态 | 理由 |
|---|---|---|---|
| 内核 & 领域 | **Rust** | **已定**（ADR-0003） | 一对三端（Tauri 2 桌面+移动 + 服务端二进制）；性能与原生能力；与 `src-tauri` 一致 |
| 插件 ABI（默认代码档） | **TypeScript**（内核内嵌 **QuickJS/rquickjs** 引擎；UI 侧直接在 webview） | **已定**（ADR-0005 / ADR-0007） | 作者门槛最低、与 UI 同语言、全端可移植 |
| 插件 ABI（性能/特权档） | 原生：进程内库（白名单）**或插件自有 CLI/守护进程** | **已定**（ADR-0005） | OS 能力；重活（索引/解析/编译）的原生性能 |
| 声明式插件 | JSON/YAML manifest | 推荐 | P6，最安全的一档（T0） |
| 外壳 UI | **TypeScript + Solid**（共享一套响应式 UI） | **已定**（ADR-0003，脚手架即此） | 桌面/移动自适应；经统一传输连任意节点 |
| 内核↔外壳契约 | 命令 + 事件流；**以 Rust 类型为源**生成 TS 类型/文档 | **已定**（Q2′） | 单源生成多端客户端（**边界之一**，见下） |
| 本地存储 | SQLite（+ 加密） + 文件 Blob | 推荐 | 单机事实源；`agent-research` 亦推荐初期 SQLite |
| 密钥存储 | OS Keychain / 等效加密 | 推荐 | 明确**不**沿用「AES 文件 + 同目录明文密钥」的反面做法 |
| 跨机传输 | WebSocket + HTTP | 推荐（v1 限同机/同网） | 对齐 `agent-research` 对 OpenCode 骨架的判断 |
| 向量/检索 | 本地向量库 + 独立 BM25 通道 | 推荐 | 见 `agent-research` 搜索线 MVP 最小集 |

### 6.1 跨语言边界有两处，不是一处

此前文档称「跨语言边界收敛到一处」，漏算了插件侧：

| # | 边界 | 机制 | 契约来源 |
|---|---|---|---|
| **B1** | Rust 内核 ↔ TypeScript 外壳 | 命令 + 事件流（Tauri IPC / 统一传输） | 以 Rust 类型为源生成 TS |
| **B2** | Rust 宿主 ↔ 内嵌 QuickJS 上的 TypeScript 插件 | 宿主能力接口（Host Capability API） | 同样需版本化 IDL，**不是自动免费** |

> **B2 是安全扩展点、插件能力、UI 宿主桥的聚集地**——第三方代码在这里碰到宿主。ADR-0010 的「安全权威」「Principal」正是在这条边界上落地。把 B2 当成「内部调用」是错的：它跨了信任边界。

---

## 7. 与 `agent-research` 的对照（哪些结论被吸收）

| 调研结论 | 本设计如何吸收 |
|---|---|
| Client/Server + SSE + OpenAPI 多端骨架（OpenCode） | 「节点 + 外壳 + 统一传输」模型（决策 4/6、§4） |
| 执行器内核 = 状态机 + 检查点（LangGraph） | Run + Step + Checkpoint（`20-domain`、`50-distributed/handoff`） |
| MCP 作发现层，Host 自己做转译/审批/审计/沙箱 | `plugin-mcp` + `plugin-policy` + `plugin-sandbox` 分工 |
| 沙箱三档：Docker → gVisor → Firecracker | `plugin-sandbox` 的适配器档位（按端选择） |
| 记忆 MVP、BM25 独立通道、golden 评估集 | `plugin-memory` 与 `70-quality/evaluation` |
| 不要基于 AutoGen 构建（已归档） | 多 Agent 编排自研/借鉴 GroupChat 终止条件 |
| 密钥不要用「明文同目录」做法 | §6 密钥存储行 |

---

## 8. 待你确认

1. 模块粒度是否合适——是否需要把某些插件合并/拆分（影响 P10 验收单元的粗细）。
2. 「必需槽位清单」是否需要按端（桌面/服务端/移动）分档——移动端大概率没有 `process.exec` 类槽位。
