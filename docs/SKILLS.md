# Repository Skills · 使用与来源

本仓库 `.agents/skills/` 保留工程 Skills 和 **[Always UI](../.agents/skills/always-ui/SKILL.md)**。UI 设计与审查统一优先使用**仓库内固定版本**，多电脑无须全局安装；不得由不同电脑的全局 Skill 覆盖。

## 仓库内实际保留的工程 Skills

| 来源 | Skill | 用途 |
| --- | --- | --- |
| [lennney/stop-that-shit](https://github.com/lennney/stop-that-shit) | `stop-that-shit` | 防止范围蔓延、无效加固及过度验证；按需声明 change/review 模式 |
| [DietrichGebert/ponytail](https://github.com/DietrichGebert/ponytail) | `ponytail` | 常规代码任务的最小完整实现、已有能力复用与简明汇报 |
| [shadcn/improve](https://github.com/shadcn/improve) | `improve` | 仅按需使用的**代码库**审计/改进计划，不是 `$always-ui improve` |
| [mattpocock/skills](https://github.com/mattpocock/skills) | `codebase-design` | 必要时协助模块边界与代码架构 |
| [mattpocock/skills](https://github.com/mattpocock/skills) | `diagnosing-bugs` | 困难缺陷排查 |
| [mattpocock/skills](https://github.com/mattpocock/skills) | `code-review` | 按需评审差异，不等同于 Codex 自动 PR 审查 |

旧版 `.agents/skills/interaction-design-rules/SKILL.md` 已从 `main` 删除；**不得继续宣称它已安装、强制读取或为此自行恢复文件**。旧版 `AGENTS.md` / 设计系统中基于 2026-11-14 问卷的泛用审美指令已改由 Always UI 统一管理。历史来源仍能通过 Git 历史和 [Issue #54](ISSUE_54.md) 查到。

以上工程 Skills 的历史来源版本见 Git 历史及仓库 `docs/licenses/` 许可文本；它们的存在**不代表其插件 Hook / Guard 已安装或启用**。

## Always UI · 当前 UI 规范入口

- 源仓库：[handFOOLed/always-ui](https://github.com/handFOOLed/always-ui)；当前固定于 [上游提交 `40ee5a3`](https://github.com/handFOOLed/always-ui/commit/40ee5a3b97edc03f10fcb64774015ca1a8b53e39)（2026-11-22）。
- 仓库内位置：`.agents/skills/always-ui/`，包含原版 `SKILL.md`、7 份 references 和 MIT 许可证。每台电脑拉取仓库即可读取，无需 `npx skills add -g`；升级需另开 PR 审核上游版本。
- 三档设计强度：`$always-ui creative`（获授权的明显风格重设计）、`$always-ui design`（布局和信息架构）、`$always-ui improve`（保留风格的局部优化）。独立 `$always-ui check` **只审查、不改代码**；设计与审查都要求时独立执行。
- **Reference-first 是强制要求**。当前版本：
  - `creative` / `design`：先读 `design-intensity.md`、`design-archetypes.md`、`value-presets.md`、`visual-discipline.md`。
  - `improve`：先读 `design-intensity.md`、`check-catalog.md`、`value-presets.md`、`visual-discipline.md`。
  - `check`：先读 `check-catalog.md`、`visual-discipline.md`、`value-presets.md`。
  - 涉及焦点、展开、文件名、宿主拖拽时，按 `SKILL.md` 要求另读 `implementation-notes.md`。以仓库内固定 `SKILL.md` 为准，上游更新须经单独 PR 审核。
- **设计数值**：先复用当前 Zibblu Token、平台度量与现有组件 → 无合适值才按 `value-presets.md` 的分类表挑选 → 特殊例外说明理由。四项视觉纪律（颜色分工、按钮主次、装饰理由、组件经济性）是**基于证据的判断**，不是全局固定审美。
- **权威顺序**：Owner 当前要求 / 安全、隐私、无障碍、平台可用性 → [Zibblu 产品交互约束](DESIGN_SYSTEM.md)与已批准 Token → Always UI 设计/审查方法。Skill **不能取消** M1/M2/M3 安全边界、重设计未授权模块或覆盖真实微信平台行为。
- 原来的 `impeccable`、`refactoring-ui` 不再作为 Zibblu UI 工作流入口；UI 工作优先使用 Always UI，不因其他 Agent 全局安装了审美 Skill 就混合规则。

## 工程工作流与维护

1. **先读 [AGENTS.md](../AGENTS.md)** 与当前 Issue，再按任务选择 Skill；从最新 `main` 开分支。常规编码参考仓库 `ponytail` 默认 `full` 的最小完整原则；小修可用 `lite`，`ultra` 只在有明确需要时使用。
2. 易扩范围、重复防御、反复验证或须区分只读审查/实际修改时，使用仓库 `stop-that-shit`。Codex 提示词第一条非空行写 `$stop-that-shit change -- <任务>`（审查改用 `review`），随后第一步仍需读 `AGENTS.md`。宿主未识别指令时直接读取仓库 Skill 并遵循建议，不得假称 Guard 已生效。
3. UI 再选仓库内 `always-ui creative/design/improve`；独立 `check` 只审查。不要为了形式机械叠加所有 Skill；只有明确授权时才调用代码库 `improve`、额外评审 Skill 或多 Agent。
4. 完成整个授权模块后运行最小必要验收，报告实际结果；完整性校验、安全权限等必要工作不能被“最小改动”原则删除。
5. 不自动执行陌生第三方脚本。Skill 文件存在 ≠ 运行时强制保护；不得虚构 Hook/Guard、真机/UI 或 CI 验证结果。
6. 更改 vendored Skill 时核对 frontmatter、全部 references、许可证及固定上游 SHA；禁止自动追踪线上最新版本。

Always UI 许可证：[`always-ui/LICENSE`](../.agents/skills/always-ui/LICENSE)。

第三方许可证：[Stop That Shit](licenses/stop-that-shit-MIT.txt) · [Ponytail](licenses/ponytail-MIT.txt) · [Improve](licenses/improve-MIT.txt) · [Matt Pocock](licenses/mattpocock-MIT.txt)。
