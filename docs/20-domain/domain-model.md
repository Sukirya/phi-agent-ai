# 领域模型

> 状态：🌱 · 最后更新：2026-11-08
> 上游：`20-domain/context-map.md`（上下文划分）、`00-overview/glossary.md`（术语）、ADR-0002（事件溯源）
> 本文是 `context-map.md` §4 的细化：**实体、值对象、不变量、状态机、事件。**

---

## 0. 怎么读这份文档

- **聚合（Aggregate）** 是一致性边界：一个聚合内部强一致，聚合之间最终一致（经事件）。
- **不变量（Invariant）** 是「任何时候都必须成立」的断言——它们是领域层单测的第一批用例。
- **事件（Event）** 是跨聚合唯一耦合方式；命名一律「过去式 + 名词」。
- 本文用**概念模型**（不写字段类型/建表），实现细节留给各子系统文档。

> 提醒：与 `agent-research` 的记忆线对齐——事实表是 Source of Truth、失效不删除、版本链。这些落到了 §5 记忆聚合。

---

## 1. 聚合总览

```
┌─────────────────────────────────────────────────────────────────────────┐
│ ② 会话、任务与运行                                                       │
│                                                                          │
│  Session ◀──owns── Turn                                                  │
│     ▲                                                                    │
│     │ references (by id)                                                 │
│     │                                                                    │
│  Run ◀──owns── Step ◀──references── ToolInvocation                       │
│   │                                                                      │
│   └──owns── Checkpoint                                                   │
└─────────────────────────────────────────────────────────────────────────┘
         │ 引用（by id，不强一致）
         ▼
┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐
│ ③ Agent 定义  │ │ ④ 工具/技能   │ │ ⑥ 策略/审批   │ │ ⑩ 记忆       │
│ AgentDefinition│ │ Tool / Skill │ │ Policy /     │ │ MemoryRecord │
│ (版本化、不可变)│ │ Invocation   │ │ Decision /   │ │ Episode      │
└──────────────┘ └──────────────┘ │ Approval     │ └──────────────┘
                                   └──────────────┘
         │
         ▼
┌──────────────┐ ┌──────────────┐
│ ⑧ 扩展机制    │ │ ⑨ 协同        │
│ Plugin /     │ │ PeerLink /   │
│ Contribution │ │ Handoff      │
└──────────────┘ └──────────────┘
```

**聚合划分原则**：生命周期一致、且需要强一致的东西放同一聚合。Run 单独成聚合（而非塞进 Session），是因为它**长生命周期、可恢复、可换端**——这三种需求都指向「独立的事件流 + 检查点」。**Task 是数据上恒在的顶层聚合**：关联 Session、拥有 `0..*` Run；界面上可隐式（惰性创建），但**每个 Run 必须有 `taskId`**（ADR-0008 修订）。

**换端与同步的单位是 Session**（ADR-0011）：一条 Session 日志只有一个写者；换端 = 转移 `ownerNodeId` 并复制整条会话日志。Run 不单独转移。

---

## 2. 值对象（跨上下文的共享词汇）

值对象无身份，按值相等。它们是自检字段类型的地方（实现时用强类型 newtype，防「id 互串」）。

| 值对象 | 构成 | 不变量 |
|---|---|---|
| `NodeId` | 节点唯一标识 | 稳定不变；生成后不可改 |
| `TaskId` / `SessionId` / `RunId` / `StepId` / `ToolInvocationId` | 类型化 ID | 各自类型不可互换（newtype） |
| `Scope` | **归属**（`user / project / agent / run` 的组合）+ **可见范围** | 归属 ≠ 授权；可见性由宿主按 `Principal` 校验（见 §7.4） |
| `Principal` | 用户 + 项目 + Agent + Run 的组合归属 | **由宿主附加**，调用方不可伪造 |
| `ActionDigest` | 执行主体 + 目标节点 + 工作区/资源身份 + `toolId@version` + 规范化参数 + 命令相关项 | 参数或目标变化 → 审批失效（`I-6-5`） |
| `RiskProfile` | 数据修改 / 信息读取与外发 / 费用 / 代码执行与权限 | 四维独立；`sideEffect` 只是输入 |
| `EgressGrant` | 接收方 + 用途 + 内容类别 | 授权外发的最小单位（主模型/审批/嵌入/反思分别授权） |
| `Budget` | `maxSteps, maxCost, maxWallTime` | 任一超限 → 触发终止 |
| `ModelSpec` | provider + model + 参数 + 定价快照 | 调用时冻结定价，避免账单漂移 |
| `Capability` | kind + resource（如 `fs.write:/workspace`） | 显式、可列举、可撤销 |
| `ContentRef` | 内容哈希 + 存储引用 | 大对象走 Blob，不塞事件 |
| `Timestamp` | `monotonic` + `civil` 双时间 | **持久排序靠 `seq`（聚合版本），不靠时钟**；单调钟只用于进程内超时与耗时测量，墙钟只用于展示 |
| `CheckpointRef` | runId + seq + 存储引用 | 指向一次可恢复快照 |

> **双重时间（bi-temporal）**：记忆与事件都携带「发生/记录」与「生效/失效」两组时间——这是 `agent-research` 记忆 MVP 的硬要求，也用于审计。

---

## 3. 聚合 ②：会话、任务与运行（核心）

### 3.0 Task（顶层目标，聚合根）

**是什么**：一个**命名的目标**。它可跨越多个会话、持续较长时间、被指派给某台设备/某个人、并由多个 Run（甚至多个 Agent）协作完成。它是**协作 / 指派 / 换端 / 看板**的单位。

| 字段 | 说明 |
|---|---|
| `id: TaskId` | |
| `title` | 目标描述 |
| `state` | `Open / Running / Blocked / Done / Archived` |
| `parentTaskId` | 可选：任务树（多 Agent / 子任务的父） |
| `assignee` | 可选：被指派到的节点 / 人（多端 / 团队用） |
| `scope` | 绑定的 project/agent 作用域 |
| `sessionIds` | 关联的会话（0..*） |
| `implicit` | 是否由系统惰性创建的隐藏 Task（用户未命名） |
| `createdAt` | |

**与 Session / Run 的关系**：

```
   Task（目标，可跨会话、可指派）
     ├── 关联 0..* Session（交互上下文）
     └── 拥有 0..* Run（一次执行；可先建目标、后执行）
   Run 必须携带 taskId（无显式 Task 时，由首个 Run 惰性创建一个隐藏 Task）
```

**不变量**：
- `I-3-0-1` **每个 Run 恰好属于一个 Task**；`Run.taskId` 必填，`RunScheduled` 事件携带 `taskId`。
- `I-3-0-2` Task 可**比其任何单个 Session 活得更久**（跨会话）。
- `I-3-0-3` 任务树深度设上限（防多 Agent 递归失控）。
- `I-3-0-4` `Archived` 的 Task 不可再指派或新增 Run（但可读）。
- `I-3-0-5` **不使用全局默认 Task**。隐式 Task 是「一事一建」：在**首个 Run 被调度时**惰性创建（纯聊天、纯问答不创建）；模型**不得静默合并**两个 Task。

**状态机**：`Open → Running ⇄ Blocked → Done → Archived`

**「可选」的含义**：数据上恒在（`I-3-0-1`），界面上可隐式——用户**不必**先命名、建卡片、操作任务界面。跨会话 / 要指派 / 要协作时才显式建 Task。

---

### 3.1 Session（聚合根）

**是什么**：一段有边界的交互上下文，归属**一个节点**（v1 单写者）。

| 字段 | 说明 |
|---|---|
| `id: SessionId` | |
| `ownerNodeId: NodeId` | 当前写者；换端时原子转移 |
| `title` | 展示用 |
| `state` | `Active / Archived` |
| `scopeBindings` | 该会话绑定的 project/agent 作用域 |
| `turns` | 轮次集合（Turn 归属 Session） |

**不变量**：
- `I-2-1` 同一时刻 Session 的写者唯一（v1）。**换端以整个 Session 为单位**（ADR-0011）：其进行中的 Run 与全部子 Run 随它一起交接，`ownerNodeId` 原子转移；Run **不**独立转移。见 `50-distributed/handoff.md`。
- `I-2-2` Turn 的序号连续、单调递增。
- `I-2-3` Archived 的 Session 不可再追加 Turn（但可读、可回放）。

**状态机**：`Active ⇄ Archived`（可重新激活）。

### 3.2 Turn（隶属 Session）

| 字段 | 说明 |
|---|---|
| `id`, `sessionId`, `index` | |
| `userInput` | 用户输入（可含附件引用） |
| `status` | `Open / Closed / Aborted` |
| `runIds` | 本轮触发过的 Run |

**不变量**：`I-2-4` 一个 Turn 关闭时，必须有终态 Run（成功/失败/中止），不能悬空。

### 3.3 Run（聚合根，核心）

**是什么**：一次 Agent 行动循环的执行，可恢复、可换端。

| 字段 | 说明 |
|---|---|
| `id: RunId` | |
| `sessionId`, `turnId` | 归属 |
| `taskId: TaskId` | **必填**（`I-3-0-1`）；无显式 Task 时指向惰性创建的隐藏 Task |
| `parentRunId?` | 可选：父 Run；构成**执行树**（子 Run 不新建用户轮次） |
| `agentDefRef` | 引用某个 AgentDefinition 的版本 |
| `pinnedVersions` | 本次运行固定的工具/技能/循环插件版本（恢复时据此校验，见 `I-3-7`） |
| `state` | 见下方状态机 |
| `budget: Budget` | 步数/成本/超时上限（含**已预留**额度） |
| `checkpoints: CheckpointRef[]` | 可恢复点 |
| `lastSeq` | 事件序号（用于续跑/复制） |

**状态机（Run）**：

```
        ┌──────────┐
        │ Scheduled│
        └────┬─────┘
             ▼
        ┌──────────┐   超预算/用户取消/不收敛
        │ Running  │───────────────┬──────────┐
        └────┬─────┘               ▼          ▼
             │                 ┌────────┐ ┌────────┐
   需审批/需人 ▼                  │Aborted │ │ Failed │
        ┌──────────────┐         └────────┘ └────────┘
        │WaitingApproval│
        └──────┬───────┘
               │ 获批/驳回
               ├──────────────▶ Running（获批）
               └──────────────▶ Aborted（驳回）
             │
             ▼
        ┌──────────┐
        │Succeeded │
        └──────────┘

   另有正交状态：
   · HandedOff（所有权已转出，本节点不再运行）
   · AwaitingReconciliation（工具结果为「未知」，暂停依赖它的后续动作，等待核对）
```

**不变量**：
- `I-3-1` Run 终态（Succeeded/Failed/Aborted）不可再进入 Running，除非显式「重跑」创建**新** Run。
- `I-3-2` 每次进入/离开 Running 必须产生事件（可回放）。
- `I-3-3` 预算超限 → 必须终止（`Aborted`），不得「再试一次」。
- `I-3-4` HandedOff 的 Run 在本节点不可再写（所有权已转出）。
- `I-3-5` **同一 Turn 至多一个活跃的根 Run**（`parentRunId` 为空的 Run）。子 Run 可并行，但属于同一个根 Run，保留 `turnId` 关联而**不新建用户轮次**。
- `I-3-6` 根 Run 进入终态（含 Aborted）后，**有界宽限期内必须取消全部后代 Run**——不留无人管理的执行。
- `I-3-7` Run 固定 `agentDefRef` + `pinnedVersions`；恢复时若工具/技能/循环插件版本已变，必须显式报告（不静默续跑）。
- `I-3-8` 存在 `OutcomeUnknown` 的 ToolInvocation 时，Run 进入 `AwaitingReconciliation`，**不得自动重试**该调用。

### 3.4 Step（隶属 Run）

| 字段 | 说明 |
|---|---|
| `id`, `runId`, `index` | |
| `phase` | `Planning / Acting / Observing / Deciding` |
| `modelCallRef` | 本步的模型调用（可空） |
| `toolInvocationIds` | 本步触发的工具调用 |
| `observationRef` | 观察结果（大对象走 Blob） |

**不变量**：`I-4-1` Step 按 index 连续；`I-4-2` 每个 Step 至少产出一个事件。

### 3.5 Checkpoint

**是什么**：一次 Run 的可恢复快照（用于中断恢复、换端）。

| 字段 | 说明 |
|---|---|
| `runId`, `seq` | 对应到哪个事件序号 |
| `snapshotRef` | 状态快照（局部物化） |
| `createdAt` | |

> 实现路线对齐 `agent-research` 的「状态机 + 检查点」（LangGraph 思路）。检查点不必每步都做，按策略（每 N 步 / 关键动作前）生成。

---

## 4. 聚合 ③：Agent 定义（核心）

### 4.1 AgentDefinition（聚合根，版本化、不可变）

**是什么**：一个 Agent 的**数据化**声明。改定义 = 产生新版本，旧 Run 仍引用旧版本。

| 字段 | 说明 |
|---|---|
| `id` | 稳定标识 |
| `version` | 语义化；每次变更递增 |
| `role`, `systemPrompt` | 角色与系统提示 |
| `toolRefs`, `skillRefs` | 引用工具/技能（by id） |
| `modelPolicy` | 模型选择策略（可交给 router） |
| `budgetDefaults: Budget` | 默认预算 |
| `memoryScope` | 记忆作用域 |

**不变量**：
- `I-3b-1` 定义是**数据**，不是代码（P6）。
- `I-3b-2` 已发布的版本不可原地修改；改 = 新版本（版本链）。
- `I-3b-3` Run 通过 `agentDefRef` 固定到某个版本——**升级定义不影响进行中的 Run**。

> 这条很重要：它让「Agent 定义随时可增删改」不破坏「进行中的任务可继续」。

### 4.2 LoopStrategy（策略，非实体）

行动循环是**可插拔算法**（`strategy.loop` 扩展点），不是领域实体。它消费 Run 的上下文、产出 Step 与工具调用。不同策略（ReAct / plan-execute / reflexion）共享同一 `Run`/`Step` 骨架。

---

## 5. 聚合 ④：工具与调用（核心）

### 5.1 ToolDefinition（声明）

| 字段 | 说明 |
|---|---|
| `id`, `version` | |
| `inputSchema`, `outputSchema` | 结构化契约 |
| `sideEffect` | `none / reversible / irreversible` ← **只是声明，不是判决**；最终风险由宿主的 `RiskProfile` 判定（ADR-0010） |
| `providerRef` | 执行体来自哪个插件（T1/T2） |

### 5.2 ToolInvocation（聚合根，核心）

**是什么**：一次工具调用的全生命周期记录。

| 字段 | 说明 |
|---|---|
| `id`, `runId`, `stepId` | |
| `toolId` | |
| `args` | 参数（大对象走 Blob） |
| `actionDigest` | 冻结后的动作摘要（`ActionDigest`）；审批绑定它（`I-6-5`） |
| `idempotencyKey` | 稳定幂等键（工具声明支持时由外部系统识别同一逻辑操作） |
| `decisionRef` | 策略裁定（allow/deny/ask） |
| `status` | 见状态机 |
| `resultRef`, `errorRef` | 结果/错误 |

**状态机**：

```
Requested ──policy──▶ Denied（终态）
    │
    ├──policy(allow)──────▶ Executing ──▶ Succeeded / Failed
    │                                    │
    │                                    └─ 可能已执行、无法查证、非幂等 ──▶ OutcomeUnknown（终态，待核对）
    │
    └──policy(ask)──▶ AwaitingApproval ──批准──▶ Executing
                                      └──驳回──▶ Denied（终态）
```

**不变量**：
- `I-5-1` **没有 PolicyDecision 就不得 Executing**（默认安全）。
- `I-5-2` `sideEffect = irreversible` 的动作默认走 `ask`（可被策略配置覆盖，但需显式）。
- `I-5-3` 外部 schema（如 MCP）经**防腐层**翻译为内部 `ToolDefinition` 后才入册。
- `I-5-4` 恢复时按工具能力选择策略：支持幂等键 → 同键重试同一逻辑操作；支持查结果 → 先查后定；可证明未执行 → 重新校验授权后重试；**可能已执行、无法查证且非幂等 → `OutcomeUnknown`，不自动重试**。
- `I-5-5` `Executing` 前必须复核：权限未撤销、审批未过期、实际资源与 `actionDigest` 一致。

---

## 6. 聚合 ⑥：策略与审批

### 6.1 PolicyRule（声明）

| 字段 | 说明 |
|---|---|
| `id`, `priority` | 优先级决定命中顺序 |
| `matcher` | 匹配动作/工具/资源/上下文 |
| `effect` | `allow / deny / ask / modify` |
| `rationale` | 可读理由（进审计） |

**不变量**：
- `I-6-1` **层内**按优先级排序，**跨层取交集**：最终允许范围 = 宿主能力 ∩ 用户授权 ∩ 当前有效策略。普通插件贡献的规则**只能收紧、不能放宽**；无任何规则命中 = 默认**拒绝**（least privilege）。
- `I-6-1b` **利益冲突**：提供某工具的插件不得贡献审批该工具的审批者实现。

### 6.2 PolicyDecision（不可变记录）

| 字段 | 说明 |
|---|---|
| `id`, `subject`(invocation), `decision`, `rationale` | |
| `by` | `rule / human / auxiliaryModel` |
| `principal` | 宿主附加的 `Principal`（判定的主体身份，不可由调用方伪造） |
| `at` | |

**不变量**：`I-6-2` 每个副作用动作恰好有一条最终决策（可有多条中间判定，以最终为准）。

### 6.3 ApprovalRequest（聚合根）

| 字段 | 说明 |
|---|---|
| `id`, `invocationId`, `status` | `Pending / Approved / Rejected / Expired` |
| `actionDigest: ActionDigest` | **冻结的被审批动作**：执行主体、目标节点、工作区/资源身份、`toolId@version`、规范化参数、命令相关项 |
| `riskProfile: RiskProfile` | 四维风险评估（数据修改 / 读取与外发 / 费用 / 代码执行与权限） |
| `approverKind` | `human / auxiliaryModel` |
| `expiresAt` | 超时策略（如 Plan 批准的 30 分钟死线，借鉴 pi-desktop 的教训） |

> **关键区分**：**辅助模型审批者不是 Agent**——它是一个无状态判定器（`judge.approver` 扩展点实现），输入「动作 + 上下文 + 风险」，输出「批准/驳回/升级给人」+ 理由。它不跑行动循环、不调工具。

**不变量**：`I-6-3` Pending 的审批到期未决 → `Expired`，对应动作视为 `Denied`（fail-safe）。
`I-6-4` 辅助模型的判定低于人类判定：小模型只可「批准低风险」，不可「否决人类已批准」，也不可批准 `irreversible`（默认策略）。
`I-6-5` **审批绑定 `actionDigest`**：参数或目标发生变化 → 旧审批**失效**，重新跑策略；执行前复核资源身份，**路径字符串摘要不能替代真实资源校验**（`I-5-5`）。

---

## 7. 聚合 ⑩：认知、记忆与反思（★核心域）

### 7.1 MemoryRecord（事实，聚合根）

| 字段 | 说明 |
|---|---|
| `id` | |
| `attribution` | **归属**：这条事实属于哪些主体（用户 / 项目 / Agent / Run 的组合） |
| `visibility` | **可见范围**：谁有权读它（由宿主按 `Principal` 校验，不由调用方传参决定） |
| `fact` | 事实内容（Source of Truth） |
| `validFrom`, `validTo` | 生效区间（bi-temporal：事实何时为真） |
| `recordedAt` | 记录时间（系统何时知道） |
| `invalidatedAt` | 撤销时间（系统何时不再采信，事务时间） |
| `supersededBy` | 被哪个新版本取代 |
| `episodeRef` | 来源原文 |
| `sourceTrust` | 来源信任等级（网页/工具返回等外部内容低于用户输入） |
| `confidence` | 置信度 |
| `state` | `Active / Invalidated / Superseded` |

**不变量（对齐 agent-research 记忆 MVP）**：
- `I-7-1` **失效 ≠ 删除**：`Invalidated` 保留，只是不再参与默认检索。
- `I-7-2` **版本链**：新事实取代旧事实时，旧记录 `supersededBy` 指向新记录，可追溯。
- `I-7-3` **写入前过准入（Admission Barrier）**：未经准入判定的内容不得成为 `MemoryRecord`。
- `I-7-4` **归属 ≠ 授权**：`attribution` 说明事实属于谁，`visibility` 说明谁能读；**可见性由宿主按 `Principal` 校验**，插件不能靠传入另一个 scope 获得权限。
- `I-7-5` **来源不可丢**：`MemoryRecord` 必须可回溯到 `episodeRef` 与 `sourceTrust`；摘要过程不得抹掉来源信任等级。

### 7.1a 记忆作用域（归属与可见性的区别）

原表把「作用域」同时当分类与授权，导致 `run` 不可读 `user` 私有的说法与「user 记忆该用户所有会话可读」冲突。现明确：

| 维度 | 回答 | 例 |
|---|---|---|
| **归属 attribution** | 这条事实**来自**谁 | 「用户在项目 P 里说的偏好」 |
| **可见范围 visibility** | 谁**有权读** | 「同一用户在当前项目的会话可读」 |

- 当前 Run 可以读取**其 Principal 有权访问**的用户偏好、项目知识、Agent 记忆——「属于 Run」不意味着只能读 run 级数据。
- **同一 Agent 用在不同项目，不自动共享项目记忆**：项目作用域以项目身份隔离。
- 扩大可见范围是**明确的共享操作**（用户发起，落事件），不是隐式继承。

### 7.1b 记忆的变更操作契约

对外不再只有 `add / search`。已承诺的生命周期必须有对应操作（否则「失效/替代/纠错/删除」只是空话）：

| 操作 | 语义 |
|---|---|
| `add` | 过准入 → 落事实/原文 |
| `search` | 双通道召回 + 作用域过滤 + Receipt |
| `supersede` | 用新事实取代旧事实（旧记录 `supersededBy` 指向新记录，状态 `Superseded`） |
| `invalidate` | 置 `Invalidated` + `invalidatedAt` + 原因（不删除） |
| `correct` | 纠错：`supersede` + 标注「纠正」原因 |
| `forget` | 用户显式删除：见 §7.1c 的跨存储删除流程 |

> 「遗忘」是 TTL/降权，不物理删；只有用户显式删除才走 `forget`。

### 7.1c 删除与脱敏（跨存储）

用户显式删除必须覆盖完整数据链，否则「删了」只是删了索引：

1. 事实记录、派生索引（向量/BM25）、Episode、Blob、检查点中的副本。
2. 未来的同步副本（已复制的节点需收到删除事件并执行）。
3. 删除本身留**审计记录**（谁在何时删了什么类别），但不保留内容。

写入侧：Episode 落盘前做**秘密识别与脱敏**（密钥、令牌、私钥）；工具结果中的秘密默认不进 Episode，只留引用。脱敏规则见 `40-runtime/context-and-memory.md`。

### 7.2 Episode

原文片段（不可变，按来源与时间归档）。Memory 是「从 Episode 提炼的事实」。

### 7.3 Reflection（反思/整理，核心域）

**是什么**：一段**离线或低优先级**的「复盘」过程——回看若干 Episode/会话，提炼出「值得沉淀的事实、可复用的经验、该失效的旧结论」，产出**候选**交给准入（Admission）判定。

| 字段 | 说明 |
|---|---|
| `id`, `scope` | 作用域 |
| `sourceRefs` | 回看哪些 Episode/Run（不可变引用） |
| `producedCandidates` | 产出的记忆候选（尚未成为 MemoryRecord） |
| `trigger` | `manual / scheduled / on-idle` |
| `at` | |

**关键约束（防「编造记忆」）**：
- `R-1` **反思只产出候选，不直接写事实**——必须过 Admission Barrier，与普通写入同一条闸门。
- `R-2` 反思**不修改权重**（它不是训练）；它改的是记忆库的候选。
- `R-3` 反思要**可追溯**：每条候选都能回溯到它的 `sourceRefs`。
- `R-4` 反思**有预算与频率上限**，不能成为「悄悄烧钱 + 悄悄污染记忆」的黑洞。

> 「dreaming / 离线整理」是这类能力的常见别名；本系统统一叫**反思（Reflection）**。它属于核心域，因为它决定 Agent 是否「越用越懂」。实现细节见 `40-runtime/context-and-memory.md` §12 待定。

---

## 8. 聚合 ⑧：扩展机制

### 8.1 Plugin（聚合根）

| 字段 | 说明 |
|---|---|
| `id`, `manifest` | 清单 |
| `tier` | `declarative / ts / native` |
| `state` | `Discovered / Resolved / Active / Failed / Unloaded` |
| `grantedCapabilities` | 授权后的能力令牌 |
| `contributions` | 已注册的贡献 |

**不变量**：`I-8-1` 未通过 `requirements` 解析的插件不得 `Active`；`I-8-2` 卸载插件必须注销其全部贡献（无残留）；`I-8-3` 插件间无直接引用（见 `module-map.md` R2）。

---

## 9. 聚合 ⑨：协同（预留）

| 实体 | 说明 |
|---|---|
| `PeerLink` | 两个节点间的连接（身份、信任、传输） |
| `Attach` | 外壳对某会话的附着（只读/可控制） |
| `Replica` | 会话副本（R2 才真正用） |
| `Handoff` | Run 所有权转移：`Offered / Accepted / Completed / Aborted` |

**不变量**：
- `I-9-1` v1 会话单写者；attach 者不写会话状态。
- `I-9-2` Handoff 是**原子**转移：接受前旧节点仍是写者，完成后新节点成为写者，旧节点转只读。
- `I-9-3` Handoff 必须携带检查点，且检查点对应的事件序号一致（否则拒绝）。
- `I-9-4` **交接单位是整个 Session**（ADR-0011）：其进行中的 Run 与全部子 Run 随它一起转移；Run 与 Task 不单独转移所有权。「指派 Task」是组织与协作，不等于搬迁其关联的所有 Session。

---

## 10. 状态机汇总

| 实体 | 状态 | 终态 |
|---|---|---|
| Task | Open / Running / Blocked / Done / Archived | Done, Archived |
| Session | Active / Archived | 无（可重激活） |
| Turn | Open / Closed / Aborted | Closed, Aborted |
| **Run** | Scheduled / Running / WaitingApproval / Succeeded / Failed / Aborted (+HandedOff、AwaitingReconciliation 正交) | Succeeded/Failed/Aborted |
| ToolInvocation | Requested / AwaitingApproval / Executing / Succeeded / Failed / Denied / OutcomeUnknown | Succeeded/Failed/Denied/OutcomeUnknown |
| ApprovalRequest | Pending / Approved / Rejected / Expired | 除 Pending 外皆终态 |
| Plugin | Discovered / Resolved / Active / Failed / Unloaded | Failed/Unloaded |
| Handoff | Offered / Accepted / Completed / Aborted | Completed/Aborted |
| MemoryRecord | Active / Invalidated / Superseded | 无（保留可追溯） |

---

## 11. 一致性边界与事务

| 场景 | 一致性 | 机制 |
|---|---|---|
| 一个聚合内的状态变更 | **强一致** | 追加事件 + 同步折叠 |
| Run 结束时更新 Turn 状态 | **最终一致** | Run 发 `RunSucceeded` → Turn 订阅并关闭 |
| 策略判定与工具执行 | **同步**（热路径） | 策略在调用链上阻塞判定，结果落事件 |
| 记忆写入 | **强一致（准入）** | 准入通过才落 `MemoryRecord` |
| 换端所有权转移 | **两阶段** | Offer→Accept→Complete，带检查点校验 |
| 跨节点复制 | **最终一致** | 事件日志按 seq 复制 |
| **事件的持久化与通知** | **先落盘，后通知** | 追加日志成功后才发布给订阅者；订阅处理失败由**投影游标**重放恢复；重复投递按 `(aggregateId, aggregateVersion)` 去重 |
| **重放：重建 vs 重执行** | **只重建，不重执行** | 重放事件只**折叠读模型**，**绝不重新调用工具或写记忆**；重执行必须由显式的新命令触发 |

> **注意**：Run 与 Turn 是不同聚合，所以「Run 终态 ⇒ Turn 可关闭」是**事件驱动的最终一致**，不是同一事务。这是有意为之：长任务不能因为另一个聚合的锁而死等。

---

## 12. 领域事件目录（详细初稿）

> 分上下文列出。payload 细节待各子系统文档；这里定「发生什么」。

### 任务（②）
- `TaskCreated { taskId, title, parentTaskId? }`
- `TaskAssigned { taskId, assignee }`
- `TaskLinkedToSession { taskId, sessionId }`
- `TaskStarted { taskId }` / `TaskBlocked { taskId, reason }` / `TaskUnblocked { taskId }`
- `TaskCompleted { taskId }` / `TaskArchived { taskId }`

### 会话/运行（②）
- `SessionStarted { sessionId, ownerNodeId, scopeBindings }`
- `SessionArchived { sessionId }`
- `SessionOwnershipTransferred { sessionId, fromNode, toNode }` ← **换端单位是 Session**（ADR-0011）
- `TurnStarted { turnId, sessionId, index, userInputRef }` ← 携带**用户输入引用**（大对象走 Blob）
- `TurnCompleted { turnId, terminalRunId }`
- `RunScheduled { runId, sessionId, turnId, taskId, parentRunId?, agentDefRef, pinnedVersions }`
- `RunStarted { runId }`
- `RunImplicitTaskCreated { taskId, runId }` ← 惰性创建隐藏 Task（`I-3-0-5`）
- `StepStarted { runId, stepId, index, phase }`
- `StepCompleted { runId, stepId }`
- `RunWaitingApproval { runId, approvalId }`
- `RunResumed { runId }`
- `RunAwaitingReconciliation { runId, invocationId }` / `RunReconciled { runId, invocationId, resolution }`
- `RunCheckpointed { runId, checkpointRef }`
- `RunSucceeded { runId }` / `RunFailed { runId, error }` / `RunAborted { runId, reason }`

### 消息与内容（②·持久化事实）
> 补「重建会话所需的事实存在哪里」（原事件目录的缺口）。大对象一律走 Blob，事件只带引用。

- `MessageAppended { sessionId, turnId, messageId, role, contentRef }` ← 用户/助手消息的**权威记录**
- `ModelOutputRecorded { callId, runId, contentRef }` ← 一次模型产出的完整内容
- `ContextSnapshotRecorded { runId, stepId, contextRef, tokenCount }` ← 该步**实际送进模型的上下文**（可回放/可审计）
- `BudgetReserved { runId, reserved }` / `BudgetSettled { runId, consumed, released }` ← 调用前预留、调用后结算（`#10`）
- `BudgetExceeded { runId, dimension }`

### 模型（⑤）
- `ModelCallIssued { callId, runId, stepId, modelSpec, contextRef, callClass }` ← `callClass` 区分 `main / auxiliary / summarize / embed`（内部调用走受限路径，见 `#9`）
- `ModelCallReturned { callId, responseRef, usage, latency, finishReason }` ← 携带**回复内容引用**
- `ModelCallFailed { callId, error }`
- `EgressGranted { grantId, recipient, purpose, contentCategories }` / `EgressRevoked { grantId }` ← 外发授权（`#29`）

### 工具（④）
- `ToolInvokeRequested { invocationId, runId, stepId, toolId, toolVersion, argsRef, actionDigest }`
- `ToolInvokeDecided { invocationId, decision, rationale }`
- `ToolInvokeStarted { invocationId }`
- `ToolInvokeCompleted { invocationId, resultRef }`
- `ToolInvokeFailed { invocationId, error }`
- `ToolInvokeOutcomeUnknown { invocationId, idempotencyKey?, evidenceRef }` ← 结果未知，待核对（`I-5-4`）

### 策略/审批（⑥）
- `PolicyDecisionMade { decisionId, invocationId, effect, by, principal, rationale }`
- `ApprovalRequested { approvalId, invocationId, actionDigest, riskProfile, approverKind, expiresAt }`
- `ApprovalResolved { approvalId, status, by }`
- `ApprovalExpired { approvalId }` ← 到期未决，对应动作视为拒绝（`I-6-3`）

### 记忆（⑩）
- `MemoryProposed { recordId, attribution, factRef, episodeRef }`
- `MemoryAdmitted { recordId }` / `MemoryRejected { recordId, reason }`
- `MemoryInvalidated { recordId, reason, invalidatedAt }`
- `MemorySuperseded { oldId, newId }`
- `MemoryForgotten { recordId, scopeSelectors }` ← 用户显式删除（跨存储清理，见 §7.1c）

### 扩展（⑧）
- `PluginDiscovered { pluginId, tier }`
- `PluginActivated { pluginId, contributions }`
- `PluginDeactivated { pluginId }`
- `PluginFailed { pluginId, error }`

### 协同（⑨，预留）
- `PeerAttached { linkId, sessionId }`
- `PeerDetached { linkId }`
- `JournalReplicated { sessionId, fromSeq, toSeq }`
- `HandoffOffered { handoffId, sessionId, fromNode, toNode, checksum }` ← **单位是 Session**（ADR-0011）
- `HandoffCompleted { handoffId, sessionId }`
- `HandoffAborted { handoffId, reason }`

> **事件目录的补充说明**（回应「事件不足以重建状态」）：
> - **大对象一律走 Blob**：事件只带 `*Ref`（`ContentRef`）；消息正文、模型输出、上下文快照、工具结果都在 Blob。
> - **生产者身份由宿主附加**：事件信封携带 `producer`（哪个模块/插件产生）与 `principal`；**关键事件（审批、预算、策略、换端）只能由所属模块产生**，插件不得伪造成这类事件（`#8`）。
> - **聚合版本**：每个事件带 `aggregateId + aggregateVersion(seq)`；会话级订阅游标是「各聚合版本的向量」，不是单一 `seq`（`#15`）。

---

## 13. 编码 Agent 的首批示例（仅示例，非约束）

> 决策 Q4：v1 默认形态是**编码 Agent**。这只影响示例与首批插件，不影响上述模型。

| 概念 | 编码场景示例 |
|---|---|
| Tool | `fs.read`, `fs.write`, `shell.exec`, `git.commit`, `search.code` |
| `sideEffect` | `fs.read = none`，`fs.write = reversible`（有 diff），`shell.exec = irreversible`（走审批） |
| Skill | 「提交规范」「项目架构地图」「评审清单」 |
| ContextContributor | 注入 `AGENTS.md` / 项目术语表 |
| Approver | 辅助模型判断「读仓库内文件 = 自动批准；写工作区外 = 升级人类」 |

---

## 14. 待定

| 编号 | 问题 | 见 |
|---|---|---|
| Q9 | 辅助模型审批的具体判定接口与默认规则 | `open-questions.md` C 组 |
| — | Checkpoint 的生成策略（每步/N步/关键动作前）、**可插拔循环策略的可序列化状态**（`#18`） | `40-runtime/agent-loop.md`、`50-distributed/handoff.md` |
| — | 记忆的准入判定由谁执行（规则/辅助模型/人） | `40-runtime/context-and-memory.md` |
| — | 事件总线与持久日志的可靠性契约（落盘顺序、订阅恢复、投影游标，`#14`） | `50-distributed/local-first.md` |