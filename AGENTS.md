# AGENTS.md — Zibblu

本文件是 **openUrain/Zibblu** 给 Codex / Claude Code / Cursor 等 AI 编码助手的协作约束，供人类协作者参考。任何 Agent 在本仓库修改代码或审查 PR 前，先阅读本文件和当前 Issue 对应的文档。

**交互语言规则（默认强制）**：面向仓库 Owner / 用户的所有自然语言沟通默认使用**简体中文**，包括开发计划、进度、完工汇报、PR 标题及正文、审查意见、返修说明与风险总结。代码标识符、命令、文件路径、协议名称、原样错误日志和必要的技术专有名词保留英文；不要为了翻译破坏代码或接口契约。用户针对某次任务明确要求其他语言时，以当次要求为准。不要因语言要求增加额外文档或翻译工作。

> **UI 设计/审查入口**：[Zibblu 产品交互约束](docs/DESIGN_SYSTEM.md) + [Always UI](https://github.com/handFOOLed/always-ui) 的实际安装版本及必读 references。对应平台微信仍是常规交互基线；Always UI 负责设计方法、数值预设和审查，不能覆盖产品安全、平台与 Owner 决策。
>
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
5. PR 创建后检查 [GitHub Actions CI](.github/workflows/ci.yml) 和 Codex Code Review；处理真实失败及可复现的重要审查意见，有新提交时重新检查对应状态。**没有实际运行的检查不能标为通过。**

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

**目前已存在的主要路径**：

- `apps/desktop/`：Tauri + React 桌面客户端与原生 Rust 入口。
- `crates/core/`：Rust 核心客户端、数据与传输能力。
- `services/online/`：Rust 在线服务及 PostgreSQL 相关代码。
- `docs/`：产品、架构、安全、路线图、决策及 Skill 来源。
- `.agents/skills/`：仓库内保留工程 Skills（`stop-that-shit`、`ponytail`、代码库 `improve` 及 Matt Pocock 精选 Skills）；UI 专用 Always UI 从实际 Agent 安装环境加载，**不是仓库内置 Skill**。
- `.github/workflows/ci.yml`：PR 和 `main` push 的 CI 检查。
- `.github/PULL_REQUEST_TEMPLATE.md`：人类与 Agent 共用的 PR 描述模板。

- 实现任务优先使用 `$stop-that-shit change -- ...`，需要时配合 `ponytail`，以最少代码完成**全部**责任。
- `improve` 仅用于明确要求的代码库审计与改进计划；Matt Pocock 的 `code-review`、`diagnosing-bugs`、`codebase-design` 按需使用，不自动触发多 Agent 流程。
- **仓库安装 Skill 不等于已安装 Guard / Hooks**。不得宣称有运行时强制保护，除非在执行环境验证其生效。
- Skills 是执行辅助，不覆盖本文件、用户授权及 [SECURITY](docs/SECURITY.md)。

## 8. 代码与文档风格

- **UI 工作强制先读** `docs/DESIGN_SYSTEM.md`、当前 Issue 与实际安装的 **[Always UI](https://github.com/handFOOLed/always-ui) `SKILL.md`**。按当次任务选择 `$always-ui creative`、`design`、`improve`；结束时可独立用 `$always-ui check` 审查，`check` 本身不改代码。**选定模式后必须先读对应 references**，不能仅凭记忆、README 或通用审美习惯操作。安装/来源与必读表见 [Skills](docs/SKILLS.md)。
- **UI 优先级**：Owner 明确要求和安全/隐私/无障碍/原生平台行为 → Zibblu 已确认的产品交互、品牌、共享 Token 与功能契约 → Always UI 的设计与检查规则。普通交互 Windows/iPhone/iPad 分别对照相应微信；**保留 Zibblu 原创图标、Lavender、小圆角和经批准的 `FloatingField`**。文件页仅列需要下载且不可在聊天内预览的普通附件，聊天文件卡片显示真实传输进度；私密聊天深色，AI 外发授权明确。
- **数值与视觉纪律**：先复用项目现有 Token / 平台尺寸；缺失时才在 Always UI `value-presets.md` 分类表选值，必要例外说明理由。按 `visual-discipline.md` 的颜色分工、按钮层级、装饰理由与组件经济性进行设计；避免随意自创数值或将风格偏好当成无障碍缺陷。旧版 `interaction-design-rules` 已从仓库删除，不得继续调用或擅自恢复；`impeccable`、`refactoring-ui` 不作为 Zibblu UI 工作流。
- **范围和验证**：`creative` 仅在 Owner 授权明显不同视觉方向时使用；`design` 不自动允许全站换肤；`improve` 保留当前布局与识别；`check` 独立审查只报告有证据的问题。若代码尚未实现目标，在 PR 写出实际差距，不能把“规范已写”当成 UI 已完成。UI PR 附真实平台截图/录屏；无法实测标 `BLOCKED`，由 Owner 验收。危险、错误、M2 LAN-only 与 M3 AI 外发授权不可因精简文案被隐藏。
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

### 合并前的人工核对

- [ ] 查看 PR 的 [GitHub Actions CI](.github/workflows/ci.yml) 状态：`Rust + PostgreSQL`（M1 集成测试）、`React + TypeScript`（前端构建）和 `Tauri Rust (Windows)`（先构建前端产物再执行桌面 Rust 编译检查）。若检查失败或未运行，先明确原因，不得写作 PASS。
- [ ] Codex 自动审查已结束；实质性审查发现已修复或说明剩余风险；审查无发现不等于功能测试通过。
- [ ] 对照当前 Issue 确认验收证据与未验证场景，特别注意 CI **不覆盖** Windows 安装包、实际 GUI 手势和 M2 真实跨设备局域网连接。
- [ ] Owner 明确决定是否合并；**不允许 Agent 自动合并**。

分支保护、必需 Review 和 CI 门禁以 **GitHub 当时对当前公开仓库 `main` 的实际配置与 PR 合并状态** 为准，不能沿用过去私有仓库的 `Not enforced` 截图或把 Codex 普通评论误当 GitHub `APPROVED`。无论技术门禁是否生效，仍须遵守 Owner 验收与禁止自动合并的人工流程。

## 面向最终读者的产物（Artifact writing）

**产物**包括应用界面与文案、用户文档、Prompt、Skill、配置示例，以及其他交付给当前开发对话参与者之外的读者的内容。读者不知道 Issue、讨论或 Agent 的实施过程；只写他们使用产物时真正需要的信息。Issue、PR、ADR 和内部验收记录可以保留必要的决策理由与证据，但不得把这些内容无故写进最终产品。

- **围绕读者写作**：省略设计取舍的理由、对话历史、废弃备选方案、无关实现细节，以及仅用于证明需求已满足的说明。例如，不为了展示“数据删除时仍保留”这一设计目标而额外添加提示，也不为了证明功能已实现而增加计数器。
- **清除过期痕迹**：功能或设计被取消后，清理受影响界面、文档、Prompt 和 Skill 中失效的入口、描述与禁令；不要保留“不要再使用旧方案”一类无用提醒。确有实际迁移需要的兼容说明、安全要求和必须留存的历史审计/ADR 不在清理范围。
- **指令约束具体行为**：只约束接收该指令的 Agent 实际可能执行的行为。优先将过宽规则收窄，而不是不断叠加例外，例如“重命名用户可见命令前先确认”，而非“重命名任何东西前先确认，内部名称除外”。
- **界面只保留有用信息**：每个控件、状态和字符串都要帮助最终用户理解、判断或操作。避免重复表达同一状态、解释开发目标、展示仅供验收的元素，或用说明文字弥补本应清晰的交互。精简不能牺牲无障碍、安全、权限告知与必要的错误反馈；状态不能只依赖颜色传达。
- **使用用户的语言**：面向用户的命名、标签与说明采用产品用户熟悉的词汇，而非 Issue、架构或开发讨论中的内部术语。
- **主动删除无用内容**：设计变更时，在受影响范围检查既有元素和文案；无论由谁编写，只要不再服务于最终用户，就移除或改写，不因其已存在而默认保留。
- **预览与授权**：显著新增或改变 UI 元素、交互或重要文案时，在合并前通过原型、截图或预览展示给 Owner；若相关方案已在当前 Issue/需求中明确确认，视为已有授权。常规缺陷修复、复用已批准组件及小幅文案修正无需重复审批。不得因此改变本仓库原有的 PR 审查、自动合并和权限边界。
- **交付前逐项检查**：在 PR 或完工汇报中，简要核对改动区域内新增或修改的每个可见元素与文案，说明其对用户的用途，删去无法说明用途的部分。该核对属于开发交付记录，不得变成产品界面的额外文字。错误消息和日志行不要求逐条制作原型。
- **同步执行者**：有子 Agent、Worker 或专门负责 UI/文档的执行者时，将本规则传递给它们，而非只约束主 Agent。

## 最终汇报契约（How to report）

**适用范围**：Agent 对当前任务的最终完工、审查或阻断汇报。接手者可能刚从其他工作切换过来，只读前几行，也看不到你执行过的命令和工具输出。因此，先写结论，正文简短、可独立执行；默认遵守本仓库的简体中文沟通要求，保留下列固定英文标题。此契约不代替 PR 模板、Issue 记录、审计证据或安全/合并门禁。

最终消息**严格按以下顺序与标题**组织，不加冗长前言：

### 1. Outcome
用一到两句话说明当前已经成立的结果；若尚未完成，明确写出发现的问题和建议。不要把计划或推测当作已完成的事实。

### 2. What I need from you
列出每项**确实需要接手者作出的决定或批准**，逐项编号，并为每项写出建议。没有需要用户决定的事项时写“无”；不得将仓库已授权的自动审查/合并流程改成逐 PR 人工审批。

### 3. What to do first
按先后顺序编号列出下一步；每一步都必须是接手者无需追问即可直接开始的动作。若工作已完成且没有后续操作，明确写“无”。

### 4. What changed or what was learned
只保留接手者继续行动所必需的变更与发现。不要复述工作过程、失败路线、重复尝试、无关的实现细节和决策历史。需检查用户可见改动时，简要交代新增或修改的控件/文案及其用途，但不要把验收解释放进产品界面。

### 5. How sure we are
分成**三个简短列表**，严格区分证据、推断与未知：
- **已检查**：具体检查过什么、对照了哪个文件/页面，以及实际执行的命令或测试和结果；对可行动的结论提供准确的文件路径或 URL。
- **推断或建议**：哪些内容只是从已读信息推断或尚待验证的建议，不能说成事实。
- **未验证或受阻**：哪些检查无人执行；未能打开的来源及 URL/路径；被阻止的命令与阻断原因。没有时写“无”。

**真实性与可执行性**：
- 只有本次执行中**实际读取或运行**过的内容才可以陈述为已验证事实；其他内容须明确标注“推断”“建议”或“未验证”。
- 每项接手者需要据此行动的主张，都要附相应的文件路径或 URL。不要要求读者查看你不可见的工具输出才能理解结论。
- 使用日常词汇和完整句子，尽量短，但不能省掉使接手者无需追问即可行动的关键条件。
- 将此要求传递给负责产出最终汇报的子 Agent 或 Worker；不要为了满足汇报格式在产品 UI 或用户文档中添加开发过程文字。

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
