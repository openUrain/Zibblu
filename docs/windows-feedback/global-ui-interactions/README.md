# 全局 UI 交互 Windows 验收

2026-11-21 · Windows 11，Tauri / WebView2 157，100% 缩放。宽窗视口 1264 × 801，窄窗视口 534 × 711。基线来自合入 PR #100 的 `f7bafeb`。每轮启动独立客户端，使用隔离资料目录、本机环回服务和合成账号、聊天及文件。

## 第一轮 Always UI Check

按 Always UI 的 `improve` 修复后，独立使用 `check` 观察输入焦点（C11、C19、C20）、菜单命中与反馈（C11、C17）和导航选中（C02、C12）。两种模式均先读取 `check-catalog.md`、`visual-discipline.md`、`value-presets.md`，实施前另读 `design-intensity.md` 和 `implementation-notes.md`。界面沿用项目色值、描边、间距与圆角。

| Issue | 基线证据与影响 | 首轮检查与整改 |
| --- | --- | --- |
| #94 | 点文件页标题栏后，搜索框仍是 `document.activeElement`，聚焦边框持续显示。点普通空白时焦点已释放，边框仍短暂过渡。 [基线截图](before/file-outside.png) | 标题栏只释放当前文本框焦点，Windows 边框与阴影即时复原。登录页标题栏、文件页标题栏和空白区域均通过焦点断言。 [有值失焦](round1/file-filled-blurred.png) · [空值失焦](round1/file-empty-blurred.png) |
| #93 | 账户菜单三处接缝的两端共 12 个采样点未命中按钮，旧背景过渡会残留在前一行。 [基线截图](before/rail-menu.png) | 账户、聊天列表加号、输入区加号通过 48 个接缝采样。首轮发现矩形热区使键盘焦点框失去圆角，随后将焦点框移到圆角高亮层，保留完整的矩形按钮热区。 [账户](round1/menu-account.png) · [列表加号](round1/menu-chat-list-plus.png) · [输入区加号](round1/menu-composer-plus.png) |
| #91 | 三个当前页图标背后都有淡薰衣草底板。 [聊天](before/nav-chats.png) · [联系人](before/nav-contacts.png) · [文件](before/nav-files.png) | 截图核对三个入口的紫色填充和文件夹浅色内线。联系人保留开放肩线，填充闭合的头部。未选中 SVG 与基线一致。 [聊天](round1/nav-chats.png) · [联系人](round1/nav-contacts.png) · [文件](round1/nav-files.png) |

## 第二轮 Always UI Check 与 Windows 回归矩阵

重新构建并启动 Windows 客户端，按 Issue 重新截图。矩阵集中执行一次，由同一构建完成；调整自动化定位和窄窗对照后，仅续跑尚未完成的场景。

| Issue | 场景 | 结果与证据 |
| --- | --- | --- |
| #94 | 登录、文件、通讯录、聊天输入区、设置和正式弹窗 | PASS。点击标题栏、空白、下一输入框或弹窗标题后，旧输入框失去焦点，边框复原。有值标签保持上浮，空值标签回落。密码显示按钮和搜索筛选仍生效。 [登录](round2/account-inactive.png) · [文件有值](round2/file-filled-blurred.png) · [文件空值](round2/file-empty-blurred.png) · [好友弹窗](round2/dialog-filled-blurred.png) · [设置](round2/settings-empty-blurred.png) |
| #94 | 聊天、键盘、输入法和文件筛选 | PASS。Enter 发送，Shift+Enter 换行，Ctrl+V 粘贴合成文本，`@` 打开提及菜单，Escape 关闭。切换页面后聊天草稿仍在。Tab / Shift+Tab、正式弹窗关闭、文件类型下拉和文件页 Tab 切换正常。通过 WebView2 CDP 组合拼音并提交「北岸」，搜索结果匹配。 [失焦后草稿](round2/chat-draft-blurred.png) |
| #93 | 账户、聊天列表加号、输入区加号、消息右键 | PASS。72 个相邻行边缘采样均命中，正反向划过时当前可用行即时显示 PR #100 配色，禁用项无高亮且点击不执行。方向键跨过禁用项，End、Escape 与菜单操作通过，复验确认圆角键盘焦点可见。 [账户](round2/menu-account.png) · [键盘焦点](round2/menu-keyboard.png) · [列表加号](round2/menu-chat-list-plus.png) · [输入区加号](round2/menu-composer-plus.png) · [消息右键](round2/menu-message-context.png) |
| #91 | 三项主导航、宽窄窗口、连续切页和键盘激活 | PASS。三项当前页均只有图标填色，切换即时。未选中 SVG 与基线一致。宽窗 44 × 44、窄窗 53 × 44 的按钮热区与基线位置和大小一致。窄窗连续切换 6 次，Tab 焦点可见且 Enter 打开正确页面。 [宽窗聊天](round2/nav-chats.png) · [宽窗联系人](round2/nav-contacts.png) · [宽窗文件](round2/nav-files.png) · [窄窗聊天](round2/narrow-chats.png) · [窄窗联系人](round2/narrow-contacts.png) · [窄窗文件](round2/narrow-files.png) · [窄窗键盘焦点](round2/narrow-nav-keyboard.png) |
| #94 / #91 | 窄窗设置 | PASS。通过账户菜单进入设置，通用页面按窄窗层级展示。 [窄窗通用设置](round2/narrow-settings.png) |

## 构建与边界

- `npm run build` 和 Windows MSVC 下的 `cargo build --offline -p zibblu-desktop --features custom-protocol` 通过。 `node --test apps/desktop/tests/*.test.mjs` 5 项通过。
- 代码差异限于 Windows 窗口交互、导航图标和共享菜单表现。界面测试使用合成资料及环回服务，未连接云端 AI。截图不含真实账号、聊天、凭据或私密内容。
- **BLOCKED**：Windows 实体中文输入法候选窗、屏幕阅读器和 iPhone / iPad 真机交互。WebView2 组合输入、原生下拉和键盘焦点已通过各自的 Windows 测试。
