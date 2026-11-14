# 范围增补记录

> 状态：🌿 · 最后更新：2026-11-14
> 上游：原始设计文档集（2026-11-08 版，`00-overview/` 至 `80-roadmap/` 与 `adr/`）
> 用途：记录**原设计文档里没有、后续实现阶段新增**的能力。设计文档描述目标，本文件记录两者之间的增量；被移除的增补也保留，不抹掉历史。

---

## 0. 判定口径

- 只记录「原本没有的」：原始设计文档未描述的界面、协议、命令或产品能力。
- 实现细节（目录、函数、依赖）只在解释增补时点到为止，不写成实现说明。
- 每条记录：是什么、为什么加、落在哪里、当前状态、对文档的影响。
- 新增增补时先在本文件 §1 加一行，再在 §2 加一节；如果某块内容大到值得独立成篇，另起 `docs/` 新文件并在 `docs/README.md` 登记。

---

## 1. 增补一览

| # | 增补 | 类型 | 状态 | 主要位置 |
|---|---|---|---|---|
| 1 | Intelligent UI | 界面 | **已移除**（被工作台视图取代） | 残留代码已删除 |
| 2 | 工作台视图（Workbench view） | 界面 + 协议 | 已实现 | `phi-contracts/src/ui.rs`、`src/workbench/` |
| 3 | Provider 管理面 | 设置 / CLI | 后端与无头 CLI 已实现，桌面 UI 未接 | `provider_store.rs`、`api.rs`、`phi-headless` |
| 4 | 多语言（i18n） | 基础设施 | 已实现（桌面暂无切换入口） | `phi-i18n`、`src/i18n.ts` |
| 5 | 离线演示模型 | 模型 | 已实现 | `model_gateway.rs` |
| 6 | 无头 CLI | 外壳 | 已实现 | `phi-headless` |
| 7 | 文档增补规则 | 流程 | 已生效 | `AGENT.md` 第 6 条、本文件 |

---

## 2. 逐条记录

### 2.1 Intelligent UI（已移除）

- **是什么**：让模型以 OpenUI 程序作答，桌面端用一个 React 岛屿渲染交互界面；对话消息之外多出一块模型生成的 UI。
- **为什么加**：探索「不只是聊天窗口」的交互方式。
- **为什么移除**：产品定位不是聊天应用；React 岛屿与 Solid 原生外壳的技术选择冲突；模型生成的 UI 代码带来额外的安全与依赖负担。用户明确要求彻底重做，而不是打补丁。
- **移除范围**：`src/OpenUIBlock.tsx`、`src/openui.d.ts`、`scripts/generate-openui-prompt.mjs`、`src-tauri/assets/openui-chat-system-prompt.txt` 全部删除；`package.json` / `bun.lock` 中的 OpenUI 与 React 依赖清零。
- **替代方案**：工作台视图（§2.2）。
- **文档影响**：README 中的 Intelligent UI 章节删除；`AGENT.md` 的对应章节改写为 Workbench view。
- **保留这条记录的原因**：它是一次真实发生的范围变更，后续设计不应无意中走回「模型生成 UI 代码」的老路。

### 2.2 工作台视图（Workbench view）

- **是什么**：主界面改为任务/工作台为中心。Agent 通过 `ui_update` 工具提交一棵声明式视图树，桌面外壳用 Solid 组件注册表渲染在对话旁边；按钮和表单只能把一条消息发回同一会话，或打开 http(s) 链接。
- **与原始设计的区别**：原始文档（ADR-0006 / ADR-0007、`30-extension/plugin-system.md`）里的声明式视图树是给**插件 UI**（D 档）用的；这里扩展为**Agent 驱动的主任务界面**，并补齐了版本化协议与严格校验。
- **新增内容**：
  - 视图树协议 v1：`phi-contracts/src/ui.rs`；节点种类白名单、属性校验、深度/节点数/字节上限、动作白名单。
  - `ui_update` 内置工具（仅 workbench 模式暴露），视图作为 `UiViewUpdated` 事件入库并投影回会话。
  - 按 Run 记录的 workbench 模式开关，以及只在该模式下追加的系统提示词（`assets/workbench-system-prompt.txt`）。
  - Solid 组件注册表（`src/workbench/`），未知组件降级为占位提示，永不执行模型生成的代码。
  - 桌面布局：会话与工作台并排，窄屏自动堆叠。
- **文档影响**：README 新增「A workbench, not a chat window」；`AGENT.md` 新增 Workbench view 章节；原始文档的插件视图树章节不变（两者共存，用途不同）。

### 2.3 Provider 管理面

- **原始设计已有什么**：Provider / ModelSpec 抽象、每 Provider 独立密钥、密钥应走 OS Keychain 或等效加密（`40-runtime/model-gateway.md`）。
- **新增了什么**：
  - 多 Provider 档案的持久化（`providers.json`）：新增、更新、删除、切换当前 Provider。
  - 连接测试与模型列表拉取；自定义 headers、认证模式、定价和模型列表。
  - 凭据引用：`env:NAME`、`literal:VALUE`、`keychain:NAME`；读接口只返回脱敏提示。
  - 桌面命令（`api.rs`）与无头 CLI（`providers list/show/add/update/remove/use/test/models`）两套管理入口。
- **当前状态**：后端与无头 CLI 完整；桌面 UI 仍只有旧的单 Key 入口，没有 Provider 设置面。
- **与设计约束的偏差**：`keychain:` 适配器尚未实现，使用时会如实报错；`literal:` 会把密钥写入本地档案，属于开发期方案，不是最终形态。

### 2.4 多语言（i18n）

- **原始设计**：未提及界面语言。
- **新增内容**：English 语句作为消息 id，中文只存在于翻译目录；Rust 侧 `phi-i18n`（`t` / `t_args`），前端侧 `src/i18n.ts`；无头模式支持 `--lang`；桌面 UI 从 bootstrap 读取语言。
- **当前状态**：现有界面文案均有中英对照；桌面暂无语言切换控件。
- **约束**：源码、注释、标识符为英文；中文只能出现在翻译目录。

### 2.5 离线演示模型

- **原始设计**：本地模型列入路线图，但未描述没有 Provider 时的体验。
- **新增内容**：内置 mock 模型。未配置任何 Provider 时，它可以跑通完整的模型调用、工具执行与审批流程，并能在 workbench 模式下产出一份示例视图。
- **为什么加**：让「无 Key 也能离线体验」成为 README 承诺的一部分，同时保证无头测试不依赖网络。

### 2.6 无头 CLI

- **原始设计**：有 headless 外壳/服务端节点的概念（`10-architecture/module-map.md`、`80-roadmap/evolution.md`），但没有具体命令面。
- **新增内容**：`run`、`sessions`、`status`、`approve`、`cancel`、`bootstrap`、`providers` 等命令；明确的退出码；stdout 上的 JSONL 事件流；跨进程控制文件；数据目录独占锁；只读命令不加锁。
- **为什么加**：作为参考外壳，保证每个共享能力都能脱离桌面 UI 被验证（`AGENT.md` 硬规则）。

### 2.7 文档增补规则

- **新增于**：2026-11-14。
- **内容**：凡是在原始设计之外新增的能力，都必须记录进 `docs/`；默认写入本文件，内容较大时另起新文件并在 `docs/README.md` 登记。README 与 `AGENT.md` 只保留指针，不堆放清单。
- **为什么加**：避免「代码里已经有的东西，文档里查不到」，也避免每次补充都往 README/AGENT.md 里堆。

---

## 3. 维护约定

- 新增原设计之外的能力：先在 §1 加一行，再在 §2 加一节。
- 能力被移除或替换：更新状态并保留记录，写明原因和替代方案。
- 本文件记录的是**增量**；当前实现状态与缺口以代码和测试为准，需要时另行成文。
