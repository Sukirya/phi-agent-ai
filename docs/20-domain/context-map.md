# 限界上下文地图

> 状态：🌱 · 最后更新：2026-11-08
> 上游：`00-overview/glossary.md`（术语）、`10-architecture/overview.md`（分层）
> 下游：`20-domain/domain-model.md`（本文的细化：实体/状态机/不变量/事件）

DDD 的限界上下文（Bounded Context）划分。**划分依据是「语言边界」而非「技术分层」**：同一个词在两个上下文里含义不同，就该分开。

---

## 1. 上下文地图

```
        ┌────────────────────┐
        │ ① 身份与设备        │  User / Device / Node / Trust
        │ Identity & Device  │  （谁是用户、有哪些设备、彼此是否信任）
        └─────────┬──────────┘
                  │ 提供 身份 & 信任上下文
                  ▼
  ┌──────────────────────────┐        ┌──────────────────────────┐
  │ ② 会话、任务与运行         │◀──────▶│ ③ Agent 运行时            │
  │ Session/Task/Run          │  驱动   │ Agent Runtime            │
  │ 核心域 ★                 │        │ 核心域 ★                 │
  │ Session/Turn/Run/Step    │        │ AgentDef/Loop/Context    │
  └───────┬───────────┬──────┘        └───────┬───────────┬──────┘
          │           │                       │           │
          │ 产生事件   │ 需要能力                │ 需要模型    │ 需要判断
          ▼           ▼                       ▼           ▼
  ┌──────────────┐ ┌──────────────────┐ ┌───────────┐ ┌──────────────┐
  │ ⑦ 数据与事件  │ │ ④ 能力           │ │ ⑤ 模型    │ │ ⑥ 治理与安全  │
  │ Data & Event │ │ Capability       │ │ Model     │ │ Governance   │
  │ Journal/     │ │ Tool/Skill/MCP   │ │ Provider/ │ │ Policy/      │
  │ Storage/Scope│ │ /Sandbox         │ │ Route/    │ │ Approver/    │
  │ 支撑域        │ │ 核心域 ★         │ │ Budget    │ │ Capability   │
  └──────────────┘ └──────────────────┘ │ 支撑域     │ │ 支撑域        │
                                        └───────────┘ └──────────────┘

  ┌──────────────────────────┐        ┌──────────────────────────┐
  │ ⑧ 扩展机制                │        │ ⑨ 协同与分布式            │
  │ Extension                │        │ Collaboration            │
  │ Plugin/Manifest/ExtPoint │        │ Transport/Sync/Attach/   │
  │ 通用域（支撑全系统）       │        │ Handoff                  │
  └──────────────────────────┘        │ 支撑域（预留）            │
                                       └──────────────────────────┘

  ┌──────────────────────────┐
  │ ⑩ 认知、记忆与反思         │   ★核心域（产品差异点，见 open-questions Q6）
  │ Cognition, Memory &      │
  │ Reflection               │
  │ Memory/Retrieval/        │
  │ Admission/Reflection     │
  └──────────────────────────┘

  ★ 核心域 = 本产品的差异点所在；支撑域 = 必须做好但不构成差异；通用域 = 可外购/复用
```

---

## 2. 上下文清单

| 编号 | 上下文 | 类型 | 一句话职责 | 对应插件/模块 |
|---|---|---|---|---|
| ① | **身份与设备** | 通用域 | 用户、设备、节点身份与信任关系 | `plugin-sync`(信任)、内核 primitives(id) |
| ② | **会话、任务与运行** | ★核心域 | 任务/会话/轮次/运行/步骤的生命周期与事件产出 | `plugin-session` |
| ③ | **Agent 运行时** | ★核心域 | Agent 定义、行动循环、上下文构筑、编排 | `plugin-loop`/`orchestrator`/`context` |
| ④ | **能力** | ★核心域 | 工具、技能、MCP、沙箱执行 | `plugin-tools`/`skills`/`mcp`/`sandbox` |
| ⑤ | **模型** | 支撑域 | 模型提供方、路由、缓存、用量、预算 | `plugin-model-gw`/`model-provider-*` |
| ⑥ | **治理与安全** | 支撑域 | 策略、审批（人/辅助模型）、能力授权 | `plugin-policy`/`approver` |
| ⑦ | **数据与事件** | 支撑域 | 事件日志、存储、作用域、读模型 | `plugin-store`、内核 eventbus/ports |
| ⑧ | **扩展机制** | 通用域 | 插件清单、扩展点、贡献、生命周期、能力协商 | 内核 registry/lifecycle |
| ⑨ | **协同与分布式** | 支撑域（预留） | 传输、同步、attach、换端 | `plugin-transport`/`sync` |
| ⑩ | **认知、记忆与反思** | ★核心域 | 记忆的写入准入、检索、演化与反思 | `plugin-memory` |

> **说明**：核心域（②③④）是 phi-agent 的差异所在——「一个可自由增删能力、可多端流转的 Agent 运行时」。支撑域要可靠但不追求创新；通用域（①⑧）能复用就复用。

---

## 3. 上下文之间的关系类型

| 关系 | 说明 | 例 |
|---|---|---|
| **客户-供应商** | 下游依赖上游的模型 | ③ Agent运行时 → ⑤ 模型（消费 Model 接口） |
| **共享内核（Shared Kernel）** | 两上下文共享一小块模型 | ② 与 ⑦ 共享「事件」这一概念 |
| **遵奉者（Conformist）** | 下游照抄上游模型，不翻译 | ⑩ 记忆照抄 ② 的作用域枚举 |
| **防腐层（ACL）** | 上游模型不理想，下游加翻译层 | ④ 接入 **MCP 外部 schema** → 转译成本系统 Tool（ACL） |
| **开放主机服务（OHS）** | 上游提供稳定对外契约 | ⑧ 扩展机制对内提供稳定插件 API |

**需要警惕的关系**：③④⑤⑥ 之间若出现**双向依赖**，说明上下文划分错了。它们应通过 ② 的**事件**或 ⑥ 的**策略**单向协作。

---

## 4. 每个上下文的核心实体（初稿）

> 详细字段/不变量见 `20-domain/domain-model.md`。这里只列「有哪些东西、关键不变量是什么」。

### ① 身份与设备
- **User**：一个自然人。/ **Device**：一台物理设备。/ **Node**：设备上的一个运行实例（一台设备可有多个节点）。
- **Trust**：两个节点间的信任关系（是否允许 attach / 换端）。
- **Principal**：宿主附加的调用者身份（用户/项目/Agent/Run 组合），横切策略判定、事件订阅、记忆读写、UI 宿主桥（ADR-0010）。
- 不变量：节点身份稳定；信任是双向显式建立的，不可隐式继承；**Principal 不可由调用方伪造**。

### ② 会话、任务与运行 ★
- **Task**（顶层目标，数据上恒在）：可跨会话、可指派、可由多个 Run/Agent 协作；界面上可隐式。/ **Session**：交互上下文，归属一个节点（单写者，**换端单位**）。/ **Turn**：一次往返。/ **Run**：一次 Agent 执行，必属一个 Task 与一个 Session。/ **Step**：一次循环迭代。
- 不变量：一个 Run 恰好属于一个 Task 与一个 Session；Session 的写者同一时刻唯一（v1），换端以整个 Session 为单位（ADR-0011）；所有状态变化成为事件。

### ③ Agent 运行时 ★
- **AgentDefinition**：角色/提示/工具/技能/模型/预算（数据）。/ **LoopStrategy**：可插拔算法。/ **ContextAssembly**：一次模型调用的上下文快照。
- 不变量：AgentDefinition 是数据不是代码；Loop 不持有对具体模型的硬依赖（经 Model 端口）。

### ④ 能力 ★
- **Tool**：schema + 执行体。/ **ToolInvocation**：一次调用及其结果。/ **Skill**：声明包。/ **McpServer**：外部工具来源。
- 不变量：**一切副作用经策略与沙箱**；外部 schema 经防腐层翻译。

### ⑤ 模型
- **ModelProvider**：一个来源。/ **ModelSpec**：具体模型（能力/定价/上下文窗口）。/ **ModelCall**：一次调用 + 用量。/ **Route**：把任务路由到模型。
- 不变量：用量必须计量（预算依赖它）；密钥不落明文。

### ⑥ 治理与安全
- **Policy**：规则集。/ **Decision**：allow/deny/ask/modify。/ **Approver**：人 / 辅助模型。/ **Capability**：授权令牌。
- 不变量：**无策略即无副作用**（默认安全）；每次决策留痕。

### ⑦ 数据与事件
- **Event**：不可变记录。/ **Journal**：按聚合分的事件流。/ **Scope**：可见范围。/ **ReadModel**：派生视图。
- 不变量：事件不可变、追加式；读模型可丢弃重建。

### ⑧ 扩展机制
- **Plugin**：可加载单元。/ **Manifest**：声明。/ **ExtensionPoint**：贡献点。/ **Contribution**：具体贡献。/ **ApiVersion**：契约版本。
- 不变量：**无版本不加载**；插件间不直接引用（见 `module-map.md` R2）。

### ⑨ 协同与分布式（预留）
- **Link**：两节点间的连接。/ **Attach**：外壳对会话的附着。/ **Replica**：会话副本。/ **Handoff**：所有权转移。
- 不变量：v1 单写者；换端 = 原子转移所有权，带检查点。

### ⑩ 认知与记忆
- **MemoryRecord**：事实（Source of Truth）。/ **Episode**：原文片段。/ **Admission**：写入准入判定。
- 不变量：失效不删除；写入前过准入（`agent-research` 记忆 MVP）。

---

## 5. 跨上下文的**领域事件**目录（初稿）

> 事件是上下文之间**唯一的异步耦合方式**（配合 P3）。命名：`过去式 + 名词`。

| 事件 | 由谁发布 | 关心它的上下文 |
|---|---|---|
| `SessionStarted` / `SessionArchived` / `SessionOwnershipTransferred` | ② | ⑦⑨ |
| `TaskCreated` / `TaskAssigned` / `TaskCompleted` | ② | ③⑨ |
| `MessageAppended` / `ModelOutputRecorded` / `ContextSnapshotRecorded` | ② | ⑦ |
| `TurnCompleted` | ② | ⑦⑩ |
| `RunStarted` / `RunSucceeded` / `RunFailed` / `RunAborted` / `RunAwaitingReconciliation` | ②③ | ⑦⑤⑥ |
| `StepStarted` / `StepCompleted` | ③ | ⑦ |
| `ModelCallIssued` / `ModelCallReturned`(含用量) | ⑤ | ⑦⑥（预算） |
| `BudgetReserved` / `BudgetSettled` / `BudgetExceeded` | ⑤ | ②⑦ |
| `EgressGranted` / `EgressRevoked` | ⑤ | ⑦⑩ |
| `ToolInvokeRequested` / `ToolInvokeCompleted` / `ToolInvokeOutcomeUnknown` | ④ | ⑥⑦ |
| `PolicyDecisionMade` | ⑥ | ⑦④ |
| `ApprovalRequested` / `ApprovalResolved` / `ApprovalExpired` | ⑥ | ②⑦ |
| `MemoryAdmitted` / `MemoryInvalidated` / `MemoryForgotten` | ⑩ | ③⑦ |
| `PluginActivated` / `PluginDeactivated` | ⑧ | 所有（能力可用性变化） |
| `PeerAttached` / `HandoffCompleted` | ⑨ | ②⑦ |

> 事件目录是「系统能发生什么」的清单——它比类图更能刻画 Agent 运行时。**新增一种协作方式，往往就是新增一类事件 + 一个订阅者**，而不是改内核。

---

## 6. 划分结论

1. **② 会话 与 任务**：**已定 · B-lite 修订版**（`open-questions.md` Q5、ADR-0008）。新增 `Task`（命名目标，可跨会话/可指派/可协作）作为顶层；`Session` 与 `Run` 保持原义；**每个 Run 必属一个 Task**（数据上恒在，界面上可隐式，惰性创建，无全局默认 Task）。
2. **⑩ 记忆/反思**：**已定 · 升格为核心域**（`open-questions.md` Q6）。更名为「认知、记忆与反思」，独立优先级与评测。
3. **⑥ 治理**：**已定 · 保持独立上下文**（`open-questions.md` Q7）。它是横切工具/模型/记忆/插件的单一收口，语言与变更节奏都与运行时不同。
4. **① 身份与设备**：**已定 · 提供 `Principal`**（ADR-0010）。身份由宿主附加，横切策略、事件订阅、记忆、UI 桥。
