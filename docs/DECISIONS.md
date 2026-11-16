# Decision log

Small, durable decisions only. Add a dated entry when an actual choice changes; do not create an ADR document for every minor preference.

| ID | Decision | Reason | Status |
| --- | --- | --- | --- |
| D-001 | Brand is **Zibblu**; slogan **Chat freely. Share privately.** | Young, friendly identity with privacy as a user-controlled capability. | Accepted |
| D-002 | Three modules: **Chat & Files**, **LAN Private Chat**, **AI Assistants**. | Keep boundaries visible and development sequential. | Accepted |
| D-003 | Private chat is **opt-in, mutual-consent, LAN-only and ephemeral**; failure never falls back to cloud. | A private-mode label must match actual routing. | Accepted |
| D-004 | Cloud AI is invoked through custom `@name`, receives only one explicitly authorized read-only context snapshot. | Minimize disclosure and prevent modification of data. | Accepted |
| D-005 | Desktop-first; start with Tauri/React/TypeScript and Rust. | Reuse one Rust core for networking and file handling without premature platform work. | Provisional |
| D-006 | Implement an entire module, then run the smallest meaningful acceptance checks. Use stop-that-shit prompts/Guard where installed. | Keep Codex scoped and avoid overengineering. | Accepted |
| D-007 | M1 uses an authenticated PostgreSQL feed and resumable attachment chunks, with SQLite and staged transfers per local account. | One online service can deliver offline history and verify uploads without a separate relay, queue, or object store. | Accepted |
| D-008 | M2 uses private IPv4 mDNS discovery, QUIC, and per-session code comparison with the expected contact. | Both participants verify their current encrypted connection without a persistent key directory or online signaling. | Superseded by D-009 |
| D-009 | M2 uses short-lived account-bound device certificates, signed per-account approval, and a temporary room coordinated by the initiating device. Managed sites can opt in to routed private subnets. | Invitations and QR joins work across devices without sending private content through the account service. | Accepted |
| D-010 | Normal group QR admission grants access from the message sequence recorded at join. | New members see post-join messages and attachments. | Accepted |
| D-011 | M3 sends a one-use snapshot directly to a configured provider. Replies start in local memory; sharing requires a separate explicit normal-chat post. | The authorization covers a single inference request and the user controls group visibility. | Accepted |
| D-012 | Cross-platform design authority is `docs/DESIGN_SYSTEM.md`: Apple-like restraint, **Lavender** hue, iMessage-led polish and subtle Snapchat playfulness. Exact brand HEX pending visual approval. | One coherent identity across Windows, mobile and tablet without relying on another brand's assets. | Accepted |
| D-013 | **Chat and profile/settings reference WeChat**; keep existing Zibblu message bubbles. Desktop uses three panes; phone four bottom tabs; tablet two panes; maintain native platform fonts and gestures. | Preserve familiar interaction logic while avoiding destructive bubble redesign or pixel-identical platform copies. | Accepted |
| D-014 | **Files view lists only non-inline-previewable/download-required ordinary attachments.** Photos/videos live in chat media history. File cards and in-chat transfer progress follow WeChat; management and media viewer capabilities follow QQ. | Separate media discovery from downloadable documents; combine QQ efficiency with a quiet chat surface. | Accepted |
| D-015 | Normal UI is **light with subtle Lavender tint**; M2 private mode remains independently dark. Mostly flat, small-radius controls; glass only for overlays, line icons with filled selected state, natural restrained motion. | Maintain usability, legibility and consistent appearance without overdecorating. | Accepted |
| D-016 | Keep AI visually integrated in ordinary chat, with explicit one-shot authorization; forbid excess ornamentation, crowded controls and inconsistent interaction logic. | Visual simplicity must not conceal M2 safety or M3 disclosure states. | Accepted |


| D-017 | Interfaces should explain themselves through layout and purposeful controls, not excess instructional text; choose sliders, checkboxes, switches and selectors by actual state semantics; avoid redundant nested selection affordances (e.g. border plus checkbox for one state); express primary, secondary, tertiary and destructive importance through clear visual hierarchy. | Improve scanability, operation logic and visual clarity across devices while keeping critical security, error and AI-disclosure text explicit. | Accepted |

## D-018 · M1 Local-First 与临时文件投递

**2026-11-15 · Owner 已确认数据归属及附件 A 方案。** 普通聊天以按服务端和账号隔离的 SQLite 及本地已保存文件为长期数据源。服务器负责身份、权限、可靠临时投递和有期限的文件下载。设备间由用户确认迁移或从备份恢复。新设备登记覆盖后续投递及账号尚未完成接收的有效积压。此决策更新 D-007 的长期保留语义，保留 PostgreSQL 投递和分块实现。

| 微信公开行为 | Zibblu 选择 |
| --- | --- |
| 微信隐私保护指引第 1.34–1.35 节说明聊天记录留在终端，可由用户备份、删除或迁移，群名称、群成员昵称等必要资料由服务端保存。[微信隐私保护指引](https://oper.my-qcloud.com/cgi-bin/readtemplate?lang=zh_CN&s=privacy&t=weixin_agreement) | 本地保留历史、搜索、引用和已存文件。服务端长期持有账号、目录权限和投递所需的最小无正文记录，无跨设备自动漫游。 |
| 腾讯客服在 2024 年公开答复未接收消息可暂存约三天。图片、视频、文件、CDN 或已投递未下载的精确期限缺少同等级公开依据。[腾讯客服公开答复](https://www.ithome.com/0/787/941.htm) | 离线文字初始目标 72 小时。图片、视频和文件采用独立下载期限，元数据与核验完成分别确认。RFC 提案期限须经过验收及上线审批。 |
| 微信个人聊天没有已读回执，腾讯于 2026 年再次说明。[腾讯公开说明](https://www.ithome.com/0/949/153.htm) | 双方主动开启时，按 #39 已决定的范围仅展示当前单聊最后一条、由自己发出的消息状态。单勾依据真实持久化，双勾依据实际查看。 |

服务端在暂存期限内可读 M1 内容，HTTPS 提供传输保护。E2EE 需要独立协议审查及设备密钥生命周期设计。WAL、备份和代理缓存的实际保存期限属于上线披露的一部分。[安全边界](SECURITY.md)

[Issue #46](https://github.com/openUrain/Zibblu/issues/46) 在隔离的 Windows 和 Docker 环境实现 [v2 投递协议](ARCHITECTURE.md#m1-local-first-rfc)，采用新库和新本地资料结构。离线文字默认可投递 72 小时，文件、上传、设备和清理任务由各自的部署参数控制。候选生产期限、生产清理及任何旧数据重置须由 Owner 单独批准。

## Open decisions

1. Online-message encryption protocol and key lifecycle; do not claim E2EE until implemented.
2. Private file transfer, mobile clients and automatic normal-chat history roaming are deferred.

## Change format

```text
YYYY-MM-DD · D-00X · Decision
Context:
Decision:
Consequences:
```
