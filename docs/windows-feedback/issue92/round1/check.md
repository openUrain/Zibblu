# 第 1 轮 Always UI 检查

2026-11-21，真实 Windows Tauri 窗口，WebView 视口为 1264 × 801 和 534 × 711 CSS px，缩放比例为 1。用独立资料目录、环回服务和 42 个合成联系人。审查依据是实际安装的 `check-catalog.md`、`visual-discipline.md`、`value-presets.md` 和焦点相关的 `implementation-notes.md`。

| 严重程度 | 位置 | 观察 | 用户影响 | 最小修复 | 验证 |
| --- | --- | --- | --- | --- | --- |
| P1 · 功能缺陷，C13 | `main` 基线，回到通讯录 | 选中 Fixture 23 后切页，详情为空，搜索词被清空，列表回到顶部 | 用户要重新寻找并选择联系人 | 让已选 ID、搜索、滚动和已读资料随 Windows 工作区保留 | 首轮实测已恢复。[基线截图](../before/before-return.png) · [首轮宽窗](wide-row.png) |
| P2 · 设计问题，C02、C12 | `main` 基线，联系人行 | 悬停名称会盖住行底色的一部分 | 头像区和名称区看起来像两段选中状态 | 行容器负责选中和悬停，头像和名称按钮分别接收点击 | 首轮实测行底色连续。[基线悬停](../before/before-hover.png) · [首轮头像卡](avatar-popup.png) |
| P2 · 功能缺陷，C11 | 首轮构建，窄窗详情返回 | 返回时聚焦屏外的 Fixture 23，列表滚动从 120 px 跳到 787 px | 离开详情后丢失原来的浏览位置 | 可见行使用防滚动聚焦，选中行在屏外时将焦点移到搜索 | 已整改，第二轮独立复验。[首轮跳动截图](narrow-scroll-return.png) |
| P2 · 功能缺陷，C13 | 首轮构建，从资料发起聊天后回到通讯录 | 返回和发消息按钮仍处于禁用状态 | 资料恢复后无法继续使用主要操作 | 创建会话动作完成后结束其忙碌状态 | 已整改，第二轮独立复验。[首轮禁用截图](actions-disabled-after-message.png) |

首轮另验证了慢回包无法覆盖新选择，缓存资料在请求失败后可读且可重试，权限撤销后清除已选和原生缓存。强制色彩模式由真实 WebView2 的媒体仿真触发，选中行内焦点轮廓仍可辨。[缓存及错误](cached-profile-and-error.png) · [键盘焦点](keyboard-high-contrast.png)
