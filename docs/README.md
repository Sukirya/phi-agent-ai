# phi-agent 设计文档集

> 本地优先、多端协同的 Agent 客户端 —— 架构与设计文档
> 版本：v0.1（草稿，持续迭代） · 最后更新：2026-11-14

---

## 1. 这份文档集是什么

这是 `phi-agent` 的**架构与设计文档集**，用于在写第一行产品代码之前，把「这个系统应该长什么样」讲清楚，并作为后续人和 Agent 协作开发的**约束源（spec-as-architecture）**。

它遵循两条从参考资料继承来的铁律：

1. **只讲架构，不讲「用哪个库」**。出现具体实现的地方，一律单独放进「技术映射」小节，并标明是**推荐**还是**已定**。
2. **图一律用 ASCII**，保证任何地方都能看、永不失效。

与 `awesome-architecture/templates/_TEMPLATE.md` 的差异：模板描述的是**一类系统**（如「AI Agent 平台」）；本套文档描述的是**一个具体系统 phi-agent**，因此额外承担「可执行的约束」职责——它不只是给人读的，也是给后续实现 Agent 读的护栏。

---

## 2. 怎么读

按顺序读，每一层都建立在前一层之上：

```
第 0 层  为什么做、不做什么        → 00-overview/vision.md
第 0 层  不可动摇的设计原则        → 00-overview/principles.md
第 0 层  术语表（领域语言）        → 00-overview/glossary.md
第 0 层  还没定的关键决策          → 00-overview/open-questions.md   ← 讨论入口
──────────────────────────────────────────────────────────────
第 1 层  架构总纲 + 全景图          → 10-architecture/overview.md
第 1 层  模块地图与依赖规则         → 10-architecture/module-map.md
──────────────────────────────────────────────────────────────
第 2 层  领域上下文地图 + 核心实体  → 20-domain/context-map.md
第 2 层  领域模型细节               → 20-domain/domain-model.md
──────────────────────────────────────────────────────────────
第 3 层  扩展机制                   → 30-extension/plugin-system.md
第 3 层  运行时子系统               → 40-runtime/（循环/工具/模型/记忆/策略）
第 3 层  协同与分布式               → 50-distributed/（本地优先/拓扑/换端）
第 3 层  前端                       → 60-frontend/multi-frontend.md
第 3 层  质量与演进                 → 70-quality/、80-roadmap/
```

> **只想快速上手**：读 `00-overview/vision.md` + `00-overview/principles.md` + `10-architecture/overview.md` 三份即可建立全局观。
> **要动手开发**：从 `10-architecture/module-map.md` 找到你负责的模块，再读对应的第 3 层文档。

---

## 3. 目录结构

```
docs/
├── README.md                        # 本文件：文档地图、约定、状态
├── 00-overview/
│   ├── vision.md                    # 愿景、定位、用户、非目标、约束
│   ├── principles.md                # 架构原则（含"可移除性"设计测试）
│   ├── glossary.md                  # 术语表 / 领域语言
│   └── open-questions.md            # 待定决策与开放问题（讨论入口）
├── 10-architecture/
│   ├── overview.md                  # 架构总纲、C4、全景图、七大决策
│   └── module-map.md                # 模块地图、依赖规则、技术映射
├── 20-domain/
│   ├── context-map.md               # 限界上下文地图
│   └── domain-model.md              # 核心实体/事件/不变量
├── 30-extension/
│   └── plugin-system.md             # 插件系统与扩展点（含 UI 内嵌模型）
├── 40-runtime/
│   ├── agent-loop.md                # Agent 行动循环
│   ├── tools-and-skills.md          # 工具与技能
│   ├── model-gateway.md             # 模型网关与路由
│   ├── context-and-memory.md        # 上下文与记忆
│   └── policy-and-approval.md       # 策略、权限、审批
├── 50-distributed/
│   ├── local-first.md               # 本地优先的数据与状态
│   ├── topology-and-sync.md         # 多端拓扑、协同、同步
│   └── handoff.md                   # 任务换端继续（预留 R1）
├── 60-frontend/
│   └── multi-frontend.md            # 多端前端与 UI 扩展
├── 70-quality/
│   ├── quality-attributes.md        # 质量属性与取舍
│   └── evaluation.md                # 评测进 CI
├── 80-roadmap/
│   ├── evolution.md                 # MVP → v1 → 后续迭代（交付边界）
│   └── implementation-status.md     # 功能实现现状（以代码与测试为准）
└── adr/                             # 架构决策记录（ADR）
    └── README.md                    # ADR 的写作门槛与索引
```

> 文件先建骨架，**不预建空文件**；每份文档写完即在此登记状态。

---

## 4. 文档状态图例

| 图例 | 含义 |
|---|---|
| 🌱 | 草稿：已写出，内容可能大改 |
| 🌿 | 评审中：内容基本成型，等待讨论/确认 |
| 🌳 | 稳定：已确认，可作为开发约束 |
| ⏳ | 待定：依赖 `open-questions.md` 中未决问题 |
| 📝 | 待写：仅有骨架或尚未落笔 |

### 当前状态

| 文档 | 状态 | 说明 |
|---|---|---|
| `00-overview/vision.md` | 🌿 | 定位/非目标/约束 |
| `00-overview/principles.md` | 🌿 | 10 条原则（核心 P10 可移除性） |
| `00-overview/glossary.md` | 🌱 | 术语表，随讨论增补 |
| `00-overview/open-questions.md` | 🌿 | **讨论入口**（A/A′/B/B′ 已定；C 组待补） |
| `10-architecture/overview.md` | 🌿 | C4 + 七大决策 + 质量属性映射 |
| `10-architecture/module-map.md` | 🌿 | 模块清单 + 依赖规则 + 技术映射 |
| `20-domain/context-map.md` | 🌱 | 10 个限界上下文 |
| `20-domain/domain-model.md` | 🌱 | 聚合/值对象/状态机/不变量/事件 |
| `30-extension/plugin-system.md` | 🌿 | 三档 ABI + 扩展点目录 + UI 内嵌模型 |
| `40-runtime/agent-loop.md` | 🌿 | 循环/收敛/多 Agent |
| `40-runtime/tools-and-skills.md` | 🌿 | 工具/技能/MCP/沙箱 |
| `40-runtime/model-gateway.md` | 🌿 | 路由/缓存/计量/密钥 |
| `40-runtime/context-and-memory.md` | 🌿 | 上下文分层/记忆 MVP |
| `40-runtime/policy-and-approval.md` | 🌿 | 策略/审批/辅助模型 |
| `50-distributed/local-first.md` | 🌿 | 本地事实源/离线/单写者 |
| `50-distributed/topology-and-sync.md` | 🌿 | 对等节点/传输/attach |
| `50-distributed/handoff.md` | 🌿 | 换端（预留 R1） |
| `60-frontend/multi-frontend.md` | 🌿 | 外壳/区域/响应式/离线 UX |
| `70-quality/quality-attributes.md` | 🌿 | 质量属性场景与取舍 |
| `70-quality/evaluation.md` | 🌿 | 评测进 CI |
| `80-roadmap/evolution.md` | 🌿 | 交付边界与里程碑（MVP/v1/后续） |
| `80-roadmap/implementation-status.md` | 🌿 | 功能实现现状、缺口与优先级（以代码与测试为准） |
| `adr/0001`–`adr/0011` | 🌳 | 十一条关键决策（已接受） |

---

## 5. 写作与迭代约定

- **一次一个模块**：宁可一份文档写到「能被实现」，也不要十份都停在标题。但**允许任何文档随时被删改或重写**——本设计天生假设「模块会被增删」，文档也一样。
- **术语一致**：所有文档中的专有名词以 `glossary.md` 为准。引入新术语时先改 `glossary.md`，再在文中使用。这是 DDD 的 ubiquitous language 纪律（借鉴 `skills/docs/engineering/domain-modeling.md`）。
- **每个部件都要回答「为什么」**：没有「为什么」的部件就是过度设计，删掉。
- **关键决策进 ADR**：一个决策同时满足三条才写 ADR——① 难逆、② 无上下文会反直觉、③ 真有取舍。三条缺一不写（借鉴 `domain-modeling` 的 ADR 门槛）。
- **区分「已定」与「推荐」**：任何技术选型必须注明状态，避免把推荐当成约束。
- **文档即护栏**：写完后，关键不变量（如「内核无领域词汇」「可选模块可移除」「安全能力缺失即拒绝」「写端口 Null 不得伪装成功」）应能被工具或 CI 检查，或至少能被 review 清单检查。

---

## 6. 与代码仓库的关系

```
phi-agent/
├── docs/          ← 本设计文档集
├── src/ src-tauri/ ...    ← 应用代码（Tauri + Solid + Rust 脚手架）
└── （本地参考，不纳入版本控制）
    ├── agent-research/        ← 调研输入（只读参考，不改）
    ├── awesome-architecture/  ← 架构方法论参考（只读参考，不改）
    └── skills/                ← 技能格式与工程纪律参考（只读参考，不改）
```

`docs/` 是**唯一的设计事实源**。代码实现与文档冲突时，先判断哪边错了，再改一边——不允许两边各说各话。
