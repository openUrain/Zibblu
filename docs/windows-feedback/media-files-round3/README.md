# Windows Media & Files 第三轮验收

基线为 `2364f80`，包含 PR #101 与 #102。使用同一台 Windows 主机上的两个独立 Tauri / WebView2 进程、不同账号和资料目录，以及本机环回服务和隔离 PostgreSQL。测试图片、文件、消息和账号均为合成资料。

## #87 独立图片查看器

旧版的 PNG 预览先验证完整附件，再检查解码尺寸。4096 × 4096 的 RGBA 样本通过上传和文件摘要校验，却在缩略图与查看器共用的 1600 万像素检查处被拒绝。两个独立客户端均能复现，带同类 ICC 和文本元数据的小图可显示。静态 PNG 预览现允许 16,777,216 像素，压缩数据、解码内存、粘贴、头像与动画限制沿用原契约。查看器保存 PNG 原件，透明通道保留。

| Windows 验收项 | 结果 |
| --- | --- |
| PNG 发送方和接收方 | 两端显示 8 个 RGB / RGBA 样本的缩略图与原图，包括 4096 × 4096、横图、竖图、正方形、透明像素及 ICC 元数据。另经两端 Windows 文件选择器各发送一次 PNG，两端均显示缩略图。 |
| 独立窗口与主聊天 | Win32 枚举到不同 HWND。顶栏沿用主窗口的拖动区与最小化、最大化、关闭按钮。真实操作覆盖拖动、调整大小、最小化、最大化、恢复和关闭，主聊天继续操作。键盘方向键、指针中心滚轮、左键平移、双击 100%、旋转、适应窗口与全屏退出通过。 |
| 复制、保存与异常 | Windows 剪贴板中的 PNG 与文件选择器保存件均与原件 SHA-256 一致。损坏的 PNG 显示错误和重试。保存结束后按 Escape 关闭原生窗口。 |
| 普通缓存与 M2 | 客户端重启后查验 SQLite 媒体缓存。已登录时切断环回服务，切换会话仍可读取已缓存的普通历史和 PNG。两个客户端建立已验证的 LAN 会话，收发后结束，私密正文未出现在两端 SQLite 或服务 PostgreSQL。 |
| 子窗口权限 | 子窗口的私密聊天命令和对另一附件的保存请求均被拒绝。正式会话中的接收方打开图片后退出测试群，查看器随后清除图片并拒绝复制。匿名及无成员资格的真实 HTTP 请求被拒绝。 |
| 多显示器与 DPI | **BLOCKED**　实测使用单台显示设备与 100% 缩放。物理跨屏和混合 DPI 仍需硬件。 |

[基线 PNG 错误](before/png-preview.png) · [主聊天](round2/cachealice-main-with-viewer.png) · [横图](round2/cachealice-wide-window.png) · [融合后的标题栏与透明正方形 PNG](round2/image-native-chrome.png) · [竖图](round2/image-portrait-fitted.png) · [查看器窄窗](round2/image-narrow-window.png) · [损坏图](round2/image-decode-error.png) · [群撤权](round2/image-permission-revoked.png)

## #90 共享文件与 Selector

旧版按历史和任务拆为两个 Tab，操作状态与文件分离。新列表在本机按附件 ID 关联下载任务，将待上传项与历史共同分页。待上传项以持久化创建时间参与排序，发布后保留排序位置并显示实际发送时间。

| Windows 验收项 | 结果 |
| --- | --- |
| 列表与过滤 | 单一文件列表，按文件名搜索和类型筛选。搜索无匹配正确显示空状态。未混入 PNG，发送者信息仍出现在文件行。选择器鼠标悬停即时切换，键盘使用 Enter、End 和 Escape 选择与退出。 |
| 长名、窄窗和排序 | 495 × 680 的原生窗口实测长名主体末尾省略、完整 `.tar.gz`、标题提示、同高文件行和稳定的按钮。键盘和鼠标均可反转时间排序，实测两枚三角的边界位置和尺寸相同，只有颜色交换。 |
| 并行、上传暂停与重启 | 两个真实上传并行。暂停后重启恢复同一任务，从第 2,097,152 字节继续上传。完成前后顺序和附件 ID 一致。 |
| 下载失败、重试与完整性 | 隔离代理在成功接收 1,048,576 字节后注入一次确定失败。点击重试从该偏移续传。保存件 SHA-256 为 `60fbcd053441fa38f18163f021ca5ca912fb509908bad2d0e9d372b15b45c664`，与原件一致。 |
| 下载并行与排队 | 两个保存任务并行，第三个进入队列，三个保存件的 SHA-256 均与原件一致。 |
| 已保存文件 | 通过实际 Windows 默认关联应用记事本打开保存的文本。改名导致原路径失效、等长修改导致 SHA-256 不匹配，两种情况均在 Windows 实测只切换为「保存」。重新保存的文件与原件摘要一致。 |
| 文件到期 | 已同步附件在有效期内显示保存。到期后同一文件行显示过期，保存操作消失。授权账号请求文件分块返回 HTTP 410。 |

[旧版文件中心](../global-ui-interactions/round2/narrow-files.png) · [文件中心](round2/file-library.png) · [窄窗](round2/file-library-narrow.png) · [类型 Selector](round2/file-selector-open.png) · [反向排序](round2/file-sort-reversed.png) · [搜索无结果](round2/file-no-results.png) · [正在下载](round2/file-download-running.png) · [下载失败](round2/file-download-failed.png) · [下载排队](round2/file-download-queue.png) · [上传暂停](round2/file-upload-paused.png) · [文件到期](round2/file-expired.png)

[原路径失效](round2/file-resave-missing.png) · [校验不一致](round2/file-resave-hash.png)

## 两轮独立 Always UI 检查

设计阶段使用 `design` 选定媒体画布和单一文件列表。局部调优使用 `improve`。检查读取安装版的 design-intensity、design-archetypes、value-presets、visual-discipline、check-catalog 和 implementation-notes。工具栏复用现有图标和 Lavender 状态色，文件中心保留共享搜索、分隔线和平台字体，Selector 沿用边框、圆角、表面和阴影 Token。

| 轮次 | 严重度 | 位置 | 观察与影响 | 改动与验证 |
| --- | --- | --- | --- | --- |
| 1 | 高置信度设计问题 | 查看器工具栏 | 适应窗口与全屏都用四角符号，用户需要依赖悬停文案区分。 | 改成向内箭头表示适应窗口。第二轮重新构建并截图，复测两项操作。 |
| 1 | 缺陷 | 查看器焦点 | 切换上一张后焦点回到页面外，方向键下一张无响应。 | 切图后聚焦画布，复查方向键与焦点。 |
| 1 | 高置信度设计问题 | 文件行 | Word 和普通文本使用相同的蓝色文件图标。 | Word 保留蓝色文档，普通文本回退至中性文件标识。第二轮在宽、窄窗重新检查。 |
| 2 | 缺陷 | 查看器保存 | 保存对话框返回时，画布外获得焦点，Escape 关闭失效。 | 改用独立窗口级键盘监听，重新构建并启动后复测通过。 |

最终工具栏为 44 px 单行。图片按钮居中，窗口按钮贴右，标题区域用于拖动。实测 584 个逻辑像素的最窄窗口仍完整显示 11 个按钮，图片组中心与窗口中心重合。最终构建用两个新资料目录启动，复测拖动、最小化、最大化、恢复、关闭、复制和保存。拖动实际使查看器独立移动 45 × 54 px。

[最窄窗口的单行工具栏](round2/viewer-minimum-width.png)

[第一轮查看器工具栏](round1/image-toolbar.png) · [第一轮暂停任务](round1/file-upload-paused.png) · [第一轮 Selector](round1/file-selector-open.png)

## 针对性自动检查

- `npm run build` 与 Windows Tauri `cargo build -p zibblu-desktop --features custom-protocol --offline` 通过。
- 7 项前端测试通过，覆盖文件名拆分、十进制单位、媒体分类和日期规则等。
- 9 项桌面 Rust 库测试通过，包括 4K PNG alpha、宽高比及工作区计算。默认沙箱中的既有 Windows 文件路径测试单独在正常权限下复测通过。
- 本地核心测试验证待上传、失败、重启、发布后的稳定排序及双向分页游标。
