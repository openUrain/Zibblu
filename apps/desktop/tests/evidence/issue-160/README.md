# Issue #160 Windows UI 实测与视觉精修证据

## 测试环境

- 平台：Windows 11（桌面分辨率 3072×1920，200% 缩放 / DPR=2）
- 窗口尺寸：宽窗约 `1181×786` 逻辑像素（截图 `2362×1573` 物理像素），窄窗约 `609×754` 逻辑像素（截图 `1218×1509` 物理像素）
- 运行方式：真实 Tauri 2 Windows 桌面客户端（WebView2，`cargo build --features custom-protocol`），使用独立临时目录 `ZIBBLU_DATA_DIR` 与本地环回测试服务端（`http://127.0.0.1:18421`），未触碰真实账号与生产数据
- UI 设计模式：仓库内 Always UI `improve`（遵循 `docs/DESIGN_SYSTEM.md`、`value-presets.md` 与 `visual-discipline.md`）

## 改动前后对比

| 场景 | 改动前（Before） | 改动后（After） | 解决的具体问题 |
| --- | --- | --- | --- |
| 普通聊天、代码块与文件卡片 | [01-chat-ordinary.png](before/01-chat-ordinary.png) | [01-chat-ordinary.png](after/01-chat-ordinary.png) | 顶部 `Connected` 胶囊降噪为简洁 `• 已连接`；移除代码块 token 灰色方块背景改为语义着色；文件卡片增加 1px 描边并将 `保存` 按钮抬升至 `--panel` 表面增强对比 |
| 聊天信息侧栏 | [02-chat-drawer.png](before/02-chat-drawer.png) | [02-chat-drawer.png](after/02-chat-drawer.png) | 移除侧栏中「删除会话」，仅保留「清空聊天记录」（聊天列表右键「删除聊天」不受影响） |
| 底部轨道菜单与向上翻转 | [02b-menu.png](before/02b-menu.png) | [02b-menu.png](after/02b-menu.png) | 全量中文化（设置、关于 Zibblu、意见反馈、退出登录）；修复贴底菜单遮挡触发按钮的问题，空间不足时自动向上弹出 |
| Zibblu 助手主界面 | [03-builtin-assistant.png](before/03-builtin-assistant.png) | [03-builtin-assistant.png](after/03-builtin-assistant.png) | 与普通聊天统一 52px 顶栏、48px 空态与底部悬浮圆角胶囊输入框；移除宽窗常驻返回箭头；将拥挤的四控件顶栏收口为 `模型触发胶囊 + 右侧设置图标` |
| 模型/提供商切换浮层 | — | [03b-builtin-model-popover.png](after/03b-builtin-model-popover.png) | 单击顶栏模型触发器展开浮层，整合提供商切换、模型列表、获取列表、手动输入与 AI 助理设置入口 |
| 思考强度菜单 | — | [03c-builtin-effort-menu.png](after/03c-builtin-effort-menu.png) | 思考强度内嵌为输入框左侧紧凑胶囊，展开菜单向上弹出不遮挡输入框 |
| 第 2 版 AI 服务条款弹窗 | — | [03d-builtin-terms-v2.png](after/03d-builtin-terms-v2.png) | AI 条款升级至 Version 2，未接受前前端与后端均 fail-closed 拦截发送并展示新条款 |
| 接受条款后直接发送 | — | [03e-builtin-direct-submit.png](after/03e-builtin-direct-submit.png) | 接受 Version 2 条款后提交问题不再弹出重复「发送给 AI 提供商？」确认框；助手提问气泡复用普通聊天样式并隐藏冗余「仅自己可见」标签 |
| 显式关联普通聊天（固定单次范围） | — | [03f-builtin-attach-chat.png](after/03f-builtin-attach-chat.png) | 通过输入框 `+` 菜单为本次提问显式关联普通会话（展示为可移除标签），后端固定本次任务范围并禁止模型动态扩权，发送后自动清空 |
| 联系人列表分组 | [04-contacts.png](before/04-contacts.png) | [04-contacts.png](after/04-contacts.png) | `系统联系人` 分组标题与字母分组对齐，统一分隔线与字级 |
| 文件页 | [05-files.png](before/05-files.png) | [05-files.png](after/05-files.png) | 保持简洁列表层级，顶部状态与全局视觉同步 |
| 设置页导航与边框 | [06-settings-account.png](before/06-settings-account.png) | [06-settings-account.png](after/06-settings-account.png) | 设置顶栏统一为 52px，导航栏补齐右侧分隔线；`AI 助理` 图标由重复聊天气泡改为 `Bot` 图标 |
| AI 助理设置（折叠与展开） | [07-settings-ai.png](before/07-settings-ai.png) · [07b-settings-ai-expanded.png](before/07b-settings-ai-expanded.png) | [07-settings-ai.png](after/07-settings-ai.png) · [07b-settings-ai-expanded.png](after/07b-settings-ai-expanded.png) | 与其他设置页统一为 `.settings-group` 结构并移除标题装饰图标；全量中文化；修复浮动标签与提供商行碰撞问题 |
| 窄窗适配（Zibblu 助手） | — | [08-narrow-assistant.png](after/08-narrow-assistant.png) | 窄窗下自动显示返回箭头，顶栏模型触发器、消息区与胶囊输入框均无溢出或裁切 |

## 已执行的命令与结果

- `npm --prefix apps/desktop run build`（`tsc --noEmit && vite build`）：PASS
- `node --test tests/rich-message.test.mjs`（在 `apps/desktop` 目录）：5/5 PASS
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib ai_terms`：1/1 PASS（验证 Version 2 条款生效且旧版 Version 1 fail-closed）
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib ai::agent`：5/5 PASS（含 `builtin_tools_fix_scope_without_request_chat` 与 `resolve_builtin_chats_fixes_exact_ids_and_disambiguates_names`）
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib ai::tests`：6/6 PASS
- `cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --features custom-protocol`：PASS

## 未验证范围

- 未使用真实第三方云端 AI API Key 执行线上生成回答（实测仅验证条款拦截、接受后无重复弹窗直接发单与错误态展示）
- 未构建 Windows NSIS 安装包，未在安装向导页面中实测 `windows-ai-terms.txt`（已核验文件保留 UTF-8 BOM 与 CRLF 行尾）
- 未在 iPhone / iPad 真机或模拟器运行验证
