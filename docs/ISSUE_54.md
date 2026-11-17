# Issue #54 界面对照与验收

基线为 `c2bb734`，包含 Windows v0.1.5 和 iPhone 预览版。[整改任务书](https://github.com/openUrain/Zibblu/issues/54)

## 改动前审计

| 平台 | 页面 | 控件或手势 | 基线问题 | 微信对照 | Zibblu 预期行为 | 验证证据 |
| --- | --- | --- | --- | --- | --- | --- |
| Windows | 登录、聊天、资料 | 右键复制、粘贴，键盘快捷键 | 自定义菜单用 `navigator.clipboard`，浏览器会为显式读取显示权限框。编辑器已有系统粘贴事件 | PC 版的输入框可通过键盘或右键直接编辑 | 默认保留键盘粘贴，右键调用系统剪贴板。只在用户操作时读写 | [context-menu.tsx](../apps/desktop/src/context-menu.tsx) 和 [chat.tsx](../apps/desktop/src/chat.tsx)。实测用合成文字和图片分别右键/快捷键粘贴，观察权限框和草稿 |
| Windows | 主窗、联系人、会话详情 | 左上头像、列表搜索、群聊标题、右键 | 左上为品牌图标，资料覆盖在主窗之上，群标题缺成员数 | PC 版窄功能栏、列表顶端搜索与新增入口、右侧会话和资料 | 登录头像进入资料独立页面，保留会话与草稿，群人数取当前成员列表 | [旧 Windows 资料页](windows-feedback/issue39/personal-profile.png) 和 [群详情](windows-feedback/issue39/group-drawer.png)。双账号验证成员加入/退出时标题更新 |
| 三端 | 我的、个人资料和联系人资料 | 资料行、返回、相册、缩放、保存 | ProfileView 打开时清空状态。编辑与资料同处模态框，保存文字会关掉资料。文件控件只显示选中的文件名 | 手机从「我」的头像行进入「个人信息」，再进入单项编辑。返回逐层上退 | 原生选图后预览裁剪，保存成功回到资料并就地显示结果。失败保留草稿供重试 | [profile.tsx](../apps/desktop/src/profile.tsx)。验证返回/取消、保存失败重试、头像与列表一致 |
| 三端 | 联系人、资料和头像 | 重启、联网刷新、撤销授权 | 目录已由按服务/账号独立的 SQLite 保存。资料和图片经 IPC/自定义协议逐次请求，WebView 响应 `no-store` | 已打开的列表和资料可先见现有内容 | 复用原生分区，缓存优先并异步更新。联系人受限字段有有效期，失去关系时立即撤销本机可见性 | [local_sync.rs](../crates/core/src/client/local_sync.rs) 和 [media.rs](../apps/desktop/src-tauri/src/media.rs)。验证已登录断网、重启后通过认证、切账号/服务、解除联系 |
| iPhone | 聊天、搜索、资料编辑、AI 授权 | 软键盘、候选栏、返回、长按和多选 | 全壳绑定 `visualViewport.height`，搜索和 AI 依赖桌面弹框。资料层缺少返回栈 | 根页用底部导航，进入详情后在当前页独立滚动。文本编辑由系统提供粘贴菜单，消息用长按 | 导航固定在各自页面。UIKit 提供遮挡高度，只调整对应输入区和滚动区域。用整行选择显示状态 | [ios.ts](../apps/desktop/src/ios.ts) 和 [ai.tsx](../apps/desktop/src/ai.tsx)。真机录制候选栏、横竖屏、后台恢复、大字体、系统粘贴和边缘返回 |
| iPad | 聊天、通讯录、我的 | 分栏、窄窗、触控 | iOS 构建只声明 iPhone 设备族，布局固定单栏 | 微信 iPad 横屏可同时查看会话列表与会话内容 | 常规宽度列表与详情并列，紧凑窗口按层级返回，控件保持触控尺寸 | [prepare.sh](../apps/desktop/src-tauri/ios/prepare.sh) 和 [ios.css](../apps/desktop/src/ios.css)。真实 iPadOS 测横竖屏、分屏、软键盘和外接键盘 |
| Windows、iOS/iPadOS | 文件、权限、桌面图标 | 选择文件/照片、保存、分享、通知 | 普通附件已走原生文件选择，iOS 已有校验后的共享表单。头像仍用 Web 文件控件，iOS 图标以浅色填补带透明圆角的素材 | 文件和照片由系统挑选。已保存和失败使用一致的状态词 | 文件和照片均由系统挑选，保存使用平台实际动作，iOS 图标用满底色。相机、本地网络仍按系统授权 | [commands.rs](../apps/desktop/src-tauri/src/commands.rs) 和 [prepare.sh](../apps/desktop/src-tauri/ios/prepare.sh)。原生窗口验证取消选择、授权拒绝和文件保存 |

## 界面资料

- Windows · 核对 PC 微信 4.1.8 的侧栏、列表顶部搜索及新增入口、会话区域与输入区底部工具列。4.1.15 的浏览区属于内容浏览场景，当前整改使用其常规聊天路径。[4.1.8 实测截图](https://www.ithome.com/0/926/302.htm) · [4.1.15 实测截图](https://www.ithome.com/1/003/287.htm)
- iPhone · 对照开发者商店页面与实拍操作说明。保留「聊天 / 联系人 / 文件 / 我的」四个 Zibblu 入口。头像、名称和封面置于资料行，用返回键或系统边缘手势回到上一级。[微信 App Store](https://apps.apple.com/cn/app/id414478124) · [iPhone 更换头像步骤](https://www.lenovo.com.cn/wiki/article-320383.html)
- iPad · 实测报道展示横向分栏，把会话列表和聊天分置左右。本轮以该结构适配宽窗口，窄分屏使用紧凑导航。公开截图未覆盖当前版本全部 iPad 交互。[微信 iPad 横向分栏实测](https://sspai.com/post/41374)
- 原生入口 · iOS 照片选择器只返回明确选中的资产。软件键盘占用由 UIKit 提供，浮动键盘与底部停靠键盘分别处理。[Apple 照片选择器](https://developer.apple.com/documentation/PhotoKit/selecting-photos-and-videos-in-ios) · [UIKit 键盘布局](https://developer.apple.com/documentation/uikit/uikeyboardlayoutguide)

## 原生证据与复验步骤

- Windows · WebView2 原生应用接入隔离的环回服务和新 PostgreSQL 库。合成账号登录，打开测试群聊，右键复制消息和粘贴到输入框，确认文字进入草稿。点击头像，Windows 系统选择器选图，调整到 120% 并保存，资料页和左上头像就地更新。名称保存返回个人信息页，登录账号仍为原用户名。[粘贴](windows-feedback/issue54/windows-clipboard.jpg) · [缩放裁剪](windows-feedback/issue54/windows-avatar-crop.jpg) · [保存结果](windows-feedback/issue54/windows-avatar-saved.jpg)
- Windows 相机 · 用户显式点击「扫一扫 → 打开相机」后仍走 WebView2 相机许可。系统图片选择与普通剪贴板已走原生入口。
- iPhone · **BLOCKED，需真机。** 登录后在群聊、搜索、简介与 AI 授权中分别使用系统键盘和中文候选。观察顶栏、输入框和历史位置。横竖屏、调大系统字体、切后台后返回各复测一次。系统菜单粘贴文字和图片，选头像和封面后拖动并缩放，分别取消和保存，再用左缘返回逐层退出。首次打开相机时测试允许与拒绝，并验证仍能从照片选择 QR 图。
- iPad · **BLOCKED，需真机或真实 iPadOS。** 横竖屏切换会话和联系人。把窗口缩到紧凑宽度，确认当前页面和编辑草稿保留。测试左侧搜索、右侧输入、触屏长按和鼠标右键。分别使用停靠、浮动与外接键盘，确认软键盘收起后列表、输入区和底部导航回到原位。
- 联动 · 用另一账号加入/退出群聊观察人数。断开服务再重复打开已读资料，重启并通过原服务认证后复看头像和联系人。切账号/服务验证隔离，从另一客户端解除联系后验证资料收回。双机复验 M2 同意与直连路径，单次 AI 授权复验发送前的精确内容及目标服务。

Owner 按真实交互决定验收状态。
