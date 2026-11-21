# 聊天缓存与私密会话 Windows 验收

2026-11-21 · Windows 11，Tauri / WebView2。两个独立的客户端进程和资料目录，共用同一台 Windows 主机。在线服务与 PostgreSQL 运行在环回地址，私密消息经过该主机的私有 IPv4 和已认证 QUIC 连接。所有账号、文字和媒体均为合成数据。实测视口为 1264 × 801、1008 × 751 和 584 × 711。

## 第一轮 Always UI Check

用 `improve` 修复后，启动真实客户端独立进行 `check`。对照了 Always UI 的 `check-catalog.md`、`visual-discipline.md` 和 `value-presets.md`，实施前同时核对 `design-intensity.md`、`implementation-notes.md` 和现有设计系统。

| Issue | 检查项与发现 | 整改与验证 |
| --- | --- | --- |
| #86 | 初次检查中，未设有效期的普通附件能显示，但缓存表没有相应数据。离开会话即失去预览复用。 | 允许没有服务端到期时间的附件在现有成员和撤回检查下缓存。复查确认已存入图片原件、缩略图和视频分块，切回保留 180 px 卡片高度并定位最新消息。 [普通会话](round1/normal-cached.png) |
| #88 | 检查最小化接受、当前会话入口、正文隔离和私密未读。 | 两客户端只保留同一联系人入口。最小化邀请窗单次接受，重复提交无额外房间。普通模式有未读点且不显示私密正文，当前视图可切换并在结束后回到普通历史。 [最小化邀请](round1/minimized-invitation.png) · [未读提示](round1/normal-unread.png) · [回到普通聊天](round1/returned-to-normal.png) |

## 第二轮 Always UI Check 与集成矩阵

重新启动两客户端并重新截图。矩阵统一覆盖两个 Issue。恢复已保存账号和媒体查看器的复测使用同一隔离资料续跑。

| Issue | 场景 | 实际结果与截图 |
| --- | --- | --- |
| #86 | 连续切换、长会话、稳定占位 | PASS。两个普通会话连续切换六次，核对当前会话标题和正文，没有出现可见 Loading 文案。加载早期历史后切回定位最新消息。受控延迟下图片和视频使用浅灰占位，图片加载前后高度均为 180 px。 [加载中](round2/media-placeholder.png) · [加载后](round2/normal-cached.png) |
| #86 | 高清渐显、缩略图保留、失败重试 | PASS。复验发现查看器加载高清时会暂时隐藏已缓存缩略图。整改后延迟高清响应，确认缩略图持续展示且查看区尺寸一致。停止服务使原图加载失败，恢复服务后点击重试成功。 [高清等待](round2/viewer-retained-thumbnail.png) · [重试成功](round2/viewer-retry-ready.png) |
| #86 | 跨重启缓存与离线预览 | PASS。联机恢复原账号后停止环回服务，窄窗仍可打开本地普通历史、缩略图、完整图片和已获取的视频分块。 [窄窗聊天](round2/restarted-offline-thumbnail.png) · [完整图片](round2/restarted-offline-image.png) |
| #86 | 缓存边界 | PASS。核心测试在 SQLite 中覆盖 64 MiB 逐出、无到期字段、已过期、成员截止点、未知权限元数据及跨账号/服务器隔离。Windows 实测资料目录保留三类预览条目。 |
| #88 | Windows 最小化接受、幂等、单入口 | PASS。真实最小化后由应用的 Windows 邀请窗点击一次接受，主窗口直接显示私密模式。并发重放两次接受命令未建第二个房间。额外邀请窗无权读取私密正文。 [邀请窗](round2/minimized-invitation.png) · [私密模式](round2/private-mode.png) |
| #88 | 幽灵切换、普通/私密未读 | PASS。切到普通模式保留历史。收到私密正文只亮幽灵按钮，切回看到先前消息和新消息，真实前台查看后清掉未读。 [普通模式私密未读](round2/normal-unread.png) |
| #88 | 环回在线服务停止后继续 LAN、结束与重启 | PASS。服务停止期间私密 LAN 消息送达。服务恢复后，发起端和接受端分别结束不同房间，对端自动回到普通历史。重复结束、拒绝后接受、结束后重启均通过。客户端 SQLite 和服务端 PostgreSQL 均无合成私密消息。 [回到普通聊天](round2/returned-to-normal.png) |

## 未验证项目

- **BLOCKED**　完全离线的冷启动在已保存账号校验阶段被拒绝，实际验证了这一行为。恢复账号需要访问原服务并验证设备。联机验证后离线访问已缓存资料通过。
- **BLOCKED**　两台物理 Windows 主机、Wi-Fi 网卡断开并切换路由、Windows 通知中心原生通知以及 iPhone / iPad 真机。实测走的是现有 Windows 邀请窗和本机私有 IPv4 上的已认证 LAN 连接。

构建使用 `npm run build` 和 Windows MSVC 下的 Tauri 构建。核心测试 12 项通过，另单独执行并通过需要真实 IPv4/mDNS 的 LAN 同意测试。现有前端测试 5 项通过。
