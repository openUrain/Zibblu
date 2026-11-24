# Windows UI #131 / #132 / #134 真实窗口验收证据

在物理 Windows（WebView2 `Edg/158.0.0.0`，DPR = 2）上编译并启动真实 `zibblu-desktop.exe`（`--features custom-protocol`），配合隔离临时 `ZIBBLU_DATA_DIR` 与本地回环测试数据服务，采集真实 WebView2 渲染截图、DOM 几何测量与滚轮/光标验证结果。

## 1. Issue #131 — RichMessage 滚轮链与 Markdown 斜体/粗斜体

- **中英文斜体与粗斜体**：`.rich-markdown :where(em, i):not(.katex *)` 生效，`fontStyle: italic`、`fontSynthesisStyle: auto`、`fontSynthesisWeight: auto`，中文`*斜体*`、`***粗斜体***` 与英文斜体、粗斜体、删除线、行内代码、KaTeX 公式均正常渲染。
- **滚轮与横向滚动**：短代码块、长代码块、短公式、超宽公式、AI 宽表格上普通纵向滚轮均正常驱动 `.timeline` 上下滚动（`+120px` / `-120px`）；长代码块与宽表格上 `Shift + 滚轮` 与横向滚轮（`deltaX`）均正常横向滚动内部内容（`scrollLeft: 120 -> 240`）。
- **证据**：[宽窗斜体与气泡](01-chat-wide-rich-and-bubble.png) · [宽窗代码块/公式/AI表格横滚](02-chat-wide-code-math-table.png)

## 2. Issue #132 — Windows 聊天气泡最大宽度封顶与窄窗口 65% 压缩

- **默认宽窗（`1190 × 790`，聊天行宽 `759.6px`）**：短消息 `70px`（`9.2%`），长文本、长代码块、AI 回复与宽表格气泡均为 `493.7px`（`65.0%`），超宽窗（`1440 × 900`）在 `512px` 封顶，无整页横向溢出。
- **窄单栏窗（`540 × 760`，聊天行宽 `428.6px`）**：短消息 `70px`（`16.3%`），长文本、长代码块、超宽公式、AI 回复与宽表格气泡均按 `65.0%` 平滑压缩至 `278.6px`，无横向溢出或头像错位。
- **证据**：[宽窗聊天](01-chat-wide-rich-and-bubble.png) · [窄窗 65% 压缩](03-chat-narrow-65pct.png)

## 3. Issue #134 — Windows 全局标准箭头光标与 PR #138 无回归

- 实测 `getComputedStyle(el).cursor`：侧栏导航、窗口控制、会话列表行、好友资料入口、富文本超链接、代码复制按钮、附件 `+` 按钮、发送按钮均为 `default`；输入框为 `text`；消息正文保持 `user-select: text`。
- PR #138 外壳阴影与布局锚点无回归：`.workspace-content` `box-shadow` 保持 `rgba(0, 0, 0, 0.1) 0px 1px 3px 0px`，`.rail` `left: 0, width: 60`，`.windows-topbar` `left: 60, height: 36`，`.chat-list-head` `left: 60, top: 36`。

## 未覆盖范围

- 未在 iPhone / iPad 真机或真实生产在线服务环境下执行端到端验证；截图使用本地回环测试桩数据（顶栏连接指示为重连轮询态，不影响排版、滚动与光标核验）。
- M2 局域网双端真实设备私密会话未联调（深色私密样式已在本地 Chromium Webview 对照验证）。
