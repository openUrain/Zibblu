# Repository Skills · 使用与来源

本仓库 `.agents/skills/` 保存**工程辅助 Skills**，**UI 设计/审查统一使用 Owner 的 [Always UI](https://github.com/handFOOLed/always-ui)**。Always UI 目前由 Agent 的个人/全局环境安装使用，**不是本仓库 vendored Skill**；使用前必须核实安装状态、版本和实际可访问的 `SKILL.md` 及 references。

## 仓库内实际保留的工程 Skills

| 来源 | Skill | 用途 |
| --- | --- | --- |
| [lennney/stop-that-shit](https://github.com/lennney/stop-that-shit) | `stop-that-shit` | 限定任务范围、分支与变更 |
| [DietrichGebert/ponytail](https://github.com/DietrichGebert/ponytail) | `ponytail` | 最小完整工程变更 |
| [shadcn/improve](https://github.com/shadcn/improve) | `improve` | 仅按需使用的**代码库**审计/改进计划，不是 `$always-ui improve` |
| [mattpocock/skills](https://github.com/mattpocock/skills) | `codebase-design` | 必要时协助模块边界与代码架构 |
| [mattpocock/skills](https://github.com/mattpocock/skills) | `diagnosing-bugs` | 困难缺陷排查 |
| [mattpocock/skills](https://github.com/mattpocock/skills) | `code-review` | 按需评审差异，不等同于 Codex 自动 PR 审查 |

旧版 `.agents/skills/interaction-design-rules/SKILL.md` 已从 `main` 删除；**不得继续宣称它已安装、强制读取或为此自行恢复文件**。旧版 `AGENTS.md` / 设计系统中基于 2026-11-14 问卷的泛用审美指令已改由 Always UI 统一管理。历史来源仍能通过 Git 历史和 [Issue #54](ISSUE_54.md) 查到。

以上工程 Skills 的历史来源版本见 Git 历史及仓库 `docs/licenses/` 许可文本；它们的存在**不代表其插件 Hook / Guard 已安装或启用**。

## Always UI · 当前 UI 规范入口

- 源仓库：**[handFOOLed/always-ui](https://github.com/handFOOLed/always-ui)**。
- 使用前读取实际版本的 `SKILL.md`。如尚未安装，可在合适的本地/Agent 环境运行 `npx skills add handFOOLed/always-ui -g`，确认安装结果；不能从仓库文档推断用户机器已经安装。
- 三档设计强度：`$always-ui creative`（获授权的明显风格重设计）、`$always-ui design`（布局和信息架构）、`$always-ui improve`（保留风格的局部优化）。独立 `$always-ui check` **只审查、不改代码**；设计与审查都要求时独立执行。
- **Reference-first 是强制要求**。当前版本：
  - `creative` / `design`：先读 `design-intensity.md`、`design-archetypes.md`、`value-presets.md`、`visual-discipline.md`。
  - `improve`：先读 `design-intensity.md`、`check-catalog.md`、`value-presets.md`、`visual-discipline.md`。
  - `check`：先读 `check-catalog.md`、`visual-discipline.md`、`value-presets.md`。
  - 涉及焦点、展开、文件名、宿主拖拽时，按 `SKILL.md` 要求另读 `implementation-notes.md`。若上游更新，始终以运行时实际版本为准。
- **设计数值**：先复用当前 Zibblu Token、平台度量与现有组件 → 无合适值才按 `value-presets.md` 的分类表挑选 → 特殊例外说明理由。四项视觉纪律（颜色分工、按钮主次、装饰理由、组件经济性）是**基于证据的判断**，不是全局固定审美。
- **权威顺序**：Owner 当前要求 / 安全、隐私、无障碍、平台可用性 → [Zibblu 产品交互约束](DESIGN_SYSTEM.md)与已批准 Token → Always UI 设计/审查方法。Skill **不能取消** M1/M2/M3 安全边界、重设计未授权模块或覆盖真实微信平台行为。
- 原来的 `impeccable`、`refactoring-ui` 不再作为 Zibblu UI 工作流入口；UI 工作优先使用 Always UI，不因其他 Agent 全局安装了审美 Skill 就混合规则。

## 工程工作流与维护

1. 按 [AGENTS.md](../AGENTS.md) 与当前 Issue 从最新 `main` 开分支；常规实现使用 `$stop-that-shit change -- ...`，必要时配合 `ponytail`。只有明确授权时才启动代码库 `improve`、其他评审 Skills 或多 Agent 流程。
2. 完成整个授权模块后运行最小必要验收，报告实际结果；完整性校验、安全权限等必要工作不能被“最小改动”原则删除。
3. 不自动执行陌生第三方脚本。Skill 文件存在 ≠ 运行时强制保护；不得虚构 Hook/Guard、真机/UI 或 CI 验证结果。
4. 更改 vendored Skill 时核对 frontmatter、参考文件、许可证与来源；不要把 Always UI 的线上最新内容误写成仓库已固定安装的版本。

第三方许可证：[Stop That Shit](licenses/stop-that-shit-MIT.txt) · [Ponytail](licenses/ponytail-MIT.txt) · [Improve](licenses/improve-MIT.txt) · [Matt Pocock](licenses/mattpocock-MIT.txt)。
