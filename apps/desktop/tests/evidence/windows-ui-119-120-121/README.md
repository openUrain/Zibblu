# Windows UI #119 / #120 / #121 真实窗口验收证据

在物理 Windows（Rust `1.100.0`、`stable-x86_64-pc-windows-msvc`、VS 2026 Build Tools、WebView2 Runtime `158.0.4370.33`）上编译并启动真实 `zibblu-desktop.exe`（`--features custom-protocol`），配合隔离临时 `ZIBBLU_DATA_DIR` 与本地回环测试数据服务，采集 WebView2 真实渲染截图与 DOM 几何测量（DPR = 2）。

## 阴影边缘裁切核验（P2 候选）

- **修复前（`elevation-3`: `0 5px 15px rgb(0 0 0 / 0.10)`）**：
  - `.windows .workspace-content` 右侧与底部外边距 `--shell-inset` 为 `4px`，最外层 `.shell` 为 `overflow: hidden`。
  - 真实窗口 3x 四角局部放大显示：底部因 `y-offset = 5px` 超出 `4px` 边距，在底部 `4px` 缝隙内堆积明显灰带并在窗口底边骤然截断；右侧同样未能自然淡出。
  - 证据：[修复前宽窗](before/01-chat-wide.png) · [修复前四角 3x 对比](before/05-shadow-corners.png)
- **修复后（Always UI `value-presets.md` `elevation-1`: `0 1px 3px rgb(0 0 0 / 0.10)`）**：
  - 阴影最大向下延伸 `1 + 3 = 4px`、向右延伸 `3px`，全部完整落在 `4px` `--shell-inset` 内，四边均自然羽化至背景，无裁切硬边或底边灰带堆积，同时保留前景内容容器相对磨砂外壳的轻悬浮层级。
  - 证据：[修复后宽窗聊天](after/01-chat-wide.png) · [修复后四角 3x 对比](after/05-shadow-corners.png)

## 同步实测的其余改动

- **单栏与双栏切换锚点稳定**（宽窗 `1190 × 790` vs 窄窗 `667 × 783`）：
  - `.rail` 宽度保持 `60px`（`left: 0`）
  - `.windows-topbar` 保持 `left: 60, height: 36`
  - `.app-name` 保持 `left: 76, top: 0`
  - `.chat-list-head` 保持 `left: 60, top: 36`
  - `.workspace-search` 保持 `left: 72, top: 44`
  - 证据：[宽窗聊天](after/01-chat-wide.png) · [宽窗联系人](after/02-contacts-wide.png) · [窄窗聊天](after/04-chat-narrow.png)
- **本地自然周日期标签**（测试当日为周二 2026-11-24）：
  - 聊天时间线：`2025年12月20日`、`5月12日`、`11月20日`（上周五，不误显星期五）、`前天`、`昨天`、`12:12`
  - 文件中心：`昨天`、`前天`、`11月20日`、`5月12日`
  - 证据：[宽窗聊天](after/01-chat-wide.png) · [宽窗文件中心](after/03-files-wide.png)

## 未覆盖范围

- 未在 iPhone / iPad 真机或真实生产在线服务环境下执行端到端验证；截图使用本地回环测试桩数据（因此顶栏连接指示为重连轮询态，不影响布局与阴影渲染核验）。
