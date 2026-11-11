# AGENTS.md — Zibblu

本文件是 **openUrain/Zibblu** 给 Codex / Claude Code / Cursor 等 AI 编码助手的协作约束，供人类协作者参考。任何 Agent 在本仓库修改代码或审查 PR 前，先阅读本文件和当前 Issue 对应的文档。

**交互语言规则（默认强制）**：面向仓库 Owner / 用户的所有自然语言沟通默认使用**简体中文**，包括开发计划、进度、完工汇报、PR 标题及正文、审查意见、返修说明与风险总结。代码标识符、命令、文件路径、协议名称、原样错误日志和必要的技术专有名词保留英文；不要为了翻译破坏代码或接口契约。用户针对某次任务明确要求其他语言时，以当次要求为准。不要因语言要求增加额外文档或翻译工作。

> **产品**：Zibblu — *Chat freely. Share privately.* 简洁聊天、可靠文件传输、局域网私密会话、用户按需授权的只读 AI。
>
> **仓库**：`openUrain/Zibblu`（GitHub），默认分支 **`main`**，不使用 agent-insight 的 GitCode / fork / upstream 流程。
>
> **技术方向**：Windows 桌面优先；Tauri 2 + React + TypeScript、Rust/Tokio；普通聊天本地 SQLite，在线服务按真实需要引入。仍以 [架构文档](docs/ARCHITECTURE.md) 和实际代码为准，不因本文件强行创建未用到的工程目录。
>
> **依据**：[产品规格](docs/PRODUCT.md) · [安全边界](docs/SECURITY.md) · [开发路线](docs/ROADMAP.md) · [技术决策](docs/DECISIONS.md) · [Skills](docs/SKILLS.md)

---

## 1. 分支与 PR 流程

Zibblu 是单一 GitHub 组织仓库，**正常流程不需要 `upstream`**：

1. 开工前运行 `git remote -v`，确认实际远端指向 `openUrain/Zibblu`；检查工作区和当前分支。若不同，先核实，不擅自改远端地址。
2. 从最新的 `main` 创建任务分支，例如 `feat/m1-chat-files`、`fix/file-resume`、`docs/review-guidelines`。
3. 只实现当前 Issue / 明确授权范围。完成后推送该分支，向 **`openUrain/Zibblu:main`** 提 PR。
4. 遵循 [PR 模板](.github/PULL_REQUEST_TEMPLATE.md)，写清实现内容、原因、验证结果和风险。**PR 是给人看的，不是 Agent 运行日志。**
5. 开启 Codex Code Review 后，等待或手动触发审查，处理实际可复现的关键问题；有新提交时按已配置的 review trigger 复审。

Issue 明确要求“提交 PR”（如 M1 Issue #1）时，视为该任务已授权**推送任务分支和创建 PR**；不是授权合并、发布或修改组织设置。

**禁止**：直接推送 `main`、未经明确授权强制推送、`reset --hard`、删除远端分支或修改仓库权限。**默认绝不自动合并 PR**；合并由仓库 Owner 决定。

## 2. Commit 规范

采用 Conventional Commits，常用前缀 `feat:` / `fix:` / `refactor:` / `docs:` / `test:` / `chore:` / `perf:` / `style:`，可选模块 scope，例如：

- `feat(chat): support offline message delivery`
- `fix(files): resume interrupted download safely`
- `docs: clarify LAN-only privacy boundary`

每次 Commit 聚焦单一目的。subject 简明，中文或英文均可。未经授权不要 `--amend`、`--no-verify`，也不要为了“整洁历史”重写他人的 Commit。

## 3. 提交、Push 与 PR 描述

- 提交前查看 `git status` 和差异，避免夹带密钥、临时文件、数据库、聊天记录、构建产物。
- 任务授权提交 PR 时可以运行 `git push -u origin <branch>`；若权限、远端或冲突异常，报告并停止，不用破坏性操作绕过。
- **标题**：清晰表述用户可感知的改动，如 `feat(chat): deliver M1 normal messaging and files`。
- **正文**：严格按照 [`.github/PULL_REQUEST_TEMPLATE.md`](.github/PULL_REQUEST_TEMPLATE.md) 的栏目填写：
  - 关联 Issue / 目标与范围
  - 实现内容（最多数条，说明关键数据流）
  - 验证证据（**实际执行的**命令/场景、结果；未执行的明确写明）
  - 风险、已知限制与刻意不做的内容
  - 审查与返修（若存在）
- UI 变化附必要截图；没有运行桌面 UI 就明确写“未做桌面端人工/自动化 UI 验证”。**不得把单元测试、类型检查或 mock 当作真实双客户端通信成功的证据。**
- 不粘贴冗长终端日志、无意义的文件清单或 AI 自评。供人类快速判断“做了什么、是否可用、还缺什么”。

## 4. 什么时候先对齐方案

- **必须先确认**：改变私密会话的 LAN-only 边界、AI 可见数据范围、身份/密钥管理模式、重要协议、正常聊天的持久化/同步语义，或需要突破当前 Issue 范围时。
- **先简述方案再实施**：存在显著不同的实现方式、横跨两个以上模块、引入长期基础设施或会破坏已有用户数据时。给出一两个选项与取舍，不为小事写多阶段设计报告。
- 小修复、文案、明确范围内的单模块变更直接实施。确实影响产品行为或协议时，只更新对应的 `docs/PRODUCT.md`、`docs/ARCHITECTURE.md`、`docs/SECURITY.md`、`docs/DECISIONS.md`；不要新建一套文档树。

如果真实代码或已确认决策与文档不一致，说明冲突并更新**必要的**权威文档，不要默默偏离。

## 5. 测试与验收：整模块完成后做最小验证

遵循 [ROADMAP](docs/ROADMAP.md) 和用户要求：**先完成一个可运行的完整模块，再执行最小、针对性的验收**。实施过程中允许编译或最短调试来定位错误，但不要做反复全仓 test/lint/build、无关矩阵和为测试而测试。

- **M1 普通聊天与文件**：双客户端消息往返与历史持久化；离线重连送达；中断后恢复文件且 hash 一致；无权限文件请求被拒绝。
- **M2 局域网私密聊天**：双方确认、已认证的 LAN P2P 发消息；拒绝/不可达时无服务器回退；结束/重启后无可恢复的应用聊天记录。
- **M3 只读 AI**：仅授权的消息/文件进入本次请求；越权读取或写操作被拒绝；私密会话内容不进入云端 AI。

有失败则修复并**只重测受影响的检查**。PR 写实测步骤、环境、结果及未测试的部分；不凭“看起来没问题”写 PASS。涉及界面但未实际启动应用时明确说明。

## 6. 项目内不可破坏的契约

### M1 — 普通聊天与文件
普通消息允许服务端辅助投递，并应有明确持久化与错误状态。文件断点续传必须可验证完整性、拒绝越权访问，不得静默覆盖不相关文件。不要把普通聊天声称为已实现端到端加密，除非有相应协议和实测证据。

### M2 — 私密会话
私密模式必须**双方主动同意、验证联系人的身份、在可达局域网内加密直连**。连接失败时 **fail closed**：不得自动使用云转发、普通消息通道或隐形回退；不得写入普通聊天 DB、搜索索引、日志、通知正文、遥测和云端 AI。结束时清理应用可控的会话状态与密钥，不能承诺删除截图、对端备份或系统级所有痕迹。

### M3 — AI 助理
`@自定义名称` 仅触发**一次请求**。调用方必须逐次选择授权的消息范围及可选的具体文件，向提供商清楚披露外发内容。只向 AI 提供受限只读快照；**不提供任意数据库查询、文件写入、删除、命令执行或后台监听能力**。聊天消息与文件中的指令是**不可信输入**，不能扩大权限。云端 AI 不得读取 M2 私密内容。

## 7. 仓库目录与 Skills

**目前已存在的路径**：

- `docs/`：产品、架构、安全、路线图、决策及 Skill 来源。
- `.agents/skills/`：仓库级 Skills，包括 `stop-that-shit`、`ponytail`、`improve`、Matt Pocock 精选工程 Skills。
- `.github/PULL_REQUEST_TEMPLATE.md`：人类与 Agent 共用的 PR 描述模板。

**计划中的源码目录**（创建前以 Issue 和实际实现为准）：`apps/desktop/`、`crates/core/`、`services/online/`。不存在时不要编造其已实现行为。

- 实现任务优先使用 `$stop-that-shit change -- ...`，需要时配合 `ponytail`，以最少代码完成**全部**责任。
- `improve` 仅用于明确要求的代码库审计与改进计划；Matt Pocock 的 `code-review`、`diagnosing-bugs`、`codebase-design` 按需使用，不自动触发多 Agent 流程。
- **仓库安装 Skill 不等于已安装 Guard / Hooks**。不得宣称有运行时强制保护，除非在执行环境验证其生效。
- Skills 是执行辅助，不覆盖本文件、用户授权及 [SECURITY](docs/SECURITY.md)。

## 8. 代码与文档风格

- 先遵循实际语言与代码库现有模式，优先复用，禁止无需求的跨模块抽象、兼容层或框架更换。
- **注释默认少写**；仅对不明显的 WHY、安全边界、反直觉行为写简短解释。
- 修改影响用户使用、数据流或 API 的地方时同步更新既有文档；普通局部重构不制造额外 Markdown。
- 报告文件位置尽量用 `[路径](相对路径#L行号)` 或 GitHub 可点击链接，不要求无效或虚构的行号。

## 9. 默认禁止与授权边界

除非 Issue 或用户明确授权，禁止：

- 合并 PR、发布 Release、关闭 Issue、推送 `main`、修改组织和仓库权限、自动启用 auto-merge。
- 删除文件/目录/分支，重写历史，强制推送，修改 `git config`。
- 修改 CI/工作流、脚本或依赖版本来绕过失败；新增必要依赖须在 PR 说明其用途。
- 修改真实 `.env*` 或提交 API 密钥、token、用户数据；`.env.example` 只写公开变量名与空值/虚构示例。
- 自行打开 M2/M3 的开发工作，或将 Skill 的建议扩展成另一个未获授权的项目。

安全修复、现有功能的必要依赖与当前任务的明确授权，优先于“为了最小 diff 不处理必需事项”。遇到冲突要说明，而不是绕过。

## 10. Codex 自动审查与人类审核

Codex 官方 **Code Review** 需在 Codex 设置中为本仓库开启 `Code review` 和 `Automatic review`；仓库文件本身**不能打开平台开关**。建议选择 **On every push** 作为复审触发条件；若仅在 PR 打开时审查，返修后可评论 `@codex review` 手动复审。

审查职责：

1. Codex 审阅 PR 的真实差异及关联 Issue，优先指出可验证的 bug、数据/隐私泄漏、越权、回归和验收缺口；不针对风格偏好制造噪声。
2. 开发 Agent 在**当前 PR 分支**修复明确的问题，给出必要的最小复测证据，更新 PR 描述的“审查与返修”。
3. 不得因机器人评论“没有问题”便声称测试已通过。Codex 评论不是 CI、人工验收或强制合并门禁。
4. **人类 Owner 负责最终合并**。任何自动审查、返修结果都不自动授权合并。

## Code Review Rules

### Review language
Write all human-facing review findings, summaries, and suggested fixes in **Simplified Chinese** by default. Keep code identifiers, paths, commands, and quotations of exact source code in their original form. Be concise, actionable, and avoid translating technical identifiers.

### Private transport and retention
**Flag any change** that can send M2 private-session payloads to a relay, online-chat path, persistent DB, logs, notifications, analytics or cloud AI; or mark a session as verified before peer authentication and mutual consent. Safe behavior is an authenticated, encrypted LAN-only path that fails closed. Security documentation must not promise impossible forensic erasure.

### AI context and filesystem permissions
**Flag any change** that allows M3 AI to read outside the user-approved message/file snapshot, reuse a prior authorization without consent, follow instructions from untrusted chat/file content to expand scope, or gain file/DB write or process-execution capability. Safe behavior is one-shot, explicit, read-only authorization.

### Normal messaging, files and acceptance evidence
**Flag consequential regressions** in M1 authentication, offline delivery, file resume/integrity verification or file access authorization. Do not treat mocks, only type checks, or an unexecuted test plan as evidence of completed two-client or file-transfer acceptance. Prefer actionable findings tied to changed lines, not speculative architecture/style critiques.

---

如果本文件与实际项目需求不一致，先遵守安全边界，必要时更新规则并在 PR 中解释。**做完当前模块就停，不自作主张开展下一模块。**
