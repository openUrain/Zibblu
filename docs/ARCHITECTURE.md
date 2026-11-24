# Architecture — minimal target

This document describes boundaries and a proposed build order, **not** a requirement to scaffold every component on day one.

## Conceptual layout

```text
Tauri desktop UI (React / TypeScript)
  ├─ 01 Chat & Files ── persistent chat store ── online message/file service
  ├─ 02 Private Chat ─── ephemeral session ─── authenticated LAN P2P
  └─ 03 AI Assistants ─ one-shot scoped context ─ read-only provider adapter
            │
       Rust / Tokio core (explicit typed interfaces)
```

The three modules share UI primitives and contact identity but **do not share implicit access** to each other's message payloads.

## Initial implementation decisions

- Desktop: Tauri 2, React, TypeScript. Start on Windows; cross-platform only when core flows are working.
- Core: Rust + Tokio for transport, file streams and privacy-critical checks.
- Normal chat: SQLite locally; minimum online Rust service and PostgreSQL where remote messaging actually needs them.
- Private LAN: mDNS/DNS-SD discovery and Quinn peer-to-peer transport, with per-session verification by both participants.
- AI: provider configuration and a narrowly scoped `read_selected_context` flow. Providers receive the prepared snapshot, never a broad database handle or writable file system.

**Folder layout when code exists** (do not create placeholders):
```text
apps/desktop/       # Tauri UI and integration
crates/core/        # Rust transport, storage and capability boundaries
services/online/    # online service, introduced during Module 01 if needed
docs/               # living product and engineering decisions
```

Use fewer packages if implementation proves simpler.

## M1 delivery and storage

Windows 普通聊天使用 [M1 Local-First 协议](#m1-local-first-rfc)。客户端在 SQLite 维护长期记录和出站队列，在线服务使用 PostgreSQL 维护账号、权限和有期限的投递载荷。

- Axum 默认只监听环回。Docker 内监听 `0.0.0.0:7841`，宿主机仅发布环回端口。远程连接经 HTTPS 代理。[在线部署](ONLINE_DEPLOYMENT.md)
- 联系人须双方批准。普通单聊发送要求有效的联系人关系。删除联系人终止后续发送，原普通会话的本地记录仍可查看。群聊退出终止该成员的在线访问，重新加入产生新的历史截止点。
- Windows 按规范化服务地址和账号隔离资料。登录令牌和安装设备凭据分别通过用户范围 DPAPI 保护。恢复时先向原服务校验账号和绑定设备，再打开对应的 SQLite。退出清除本次登录记录；同一设备重新验证账号后可以继续待发任务。
- 文件采用固定 UUID、块偏移和 SHA-256。上传前把文件复制到应用管理的暂存目录。接收端核验后才发布保存文件并记录内容 ACK。已核验的本地副本支持离线预览和再次保存。
- 聊天列表切换先用当前登录内最近八个会话的最新历史页，随后从原有 SQLite 读取。预览在同一账号资料中按消息序号、附件 SHA-256 和预览类型或分块偏移保留对应数据，每个资料最多 64 MiB 和 256 项，优先逐出最久未用的数据。图片原件先验证 SHA-256 和格式，再生成缩略图。视频只记已获取的分块，完整保存仍走原文件流程。
- Windows 在登录会话内共享已解码的缩略图、目录头像和已授权封面。按会话、消息序号、附件 ID 和摘要区分缩略图，按账号对象及资源版本区分资料图片。离开视口的图片按最近访问次序保留在 128 项、24 MiB 的内存预算内。撤回、隐藏、会话可见范围变化、附件到期和退出登录会让对应缓存失效。
- 联系人列表先读同步目录的 SQLite 副本，详情和快捷卡共享 SQLite 资料快照及正在进行的校验。目录中的头像版本、名称或备注变化时校验对应对象，持续打开的资料每分钟校验一次。快照有效期为 24 小时，每个资料目录最多保留 256 份快照及 96 项、32 MiB 的资料图片。有效快照可在已登录后的断网期间读取，Windows 冷启动仍需服务器确认账号和设备。
- Windows 图片查看器使用独立的顶层窗口。同一进程复用一个查看窗口，由原生端绑定当前登录、打开时的普通会话及选中图片。预览复用本机媒体缓存，切图、复制和保存继续检查附件可见性及权限。
- 文件中心将待发布上传与普通附件历史一起分页。发件设备按原任务创建时间排序，发布后仍显示服务端确认的发送时间。下载状态按附件 ID 关联。Windows 从已完成的下载记录取出保存路径，校验后交由系统默认应用打开。保存件不可用时，文件行提供保存操作。
- 桌面端在普通文字写入 SQLite 后唤醒原生发件箱任务，任务独立于接收同步处理待发送和到期重试。发布回包和接收同步可分别确认同一 nonce，SQLite 仍以事务合并。
- Windows 用已认证的 HTTP 同步及页 ACK 确认连接可用。连续失败时轮询有界退避，系统报告网络恢复时可提前发起一次同步。普通文字对网络错误和服务端暂时错误实行带抖动的短退避，按会话中的入队顺序自动补发。

## Data ownership and flow

| Data | Normal chat | Private chat | AI request |
| --- | --- | --- | --- |
| Message text | persisted in normal store; online delivery as needed | memory-only for active session; never copied into normal store | bounded in-memory snapshot after user's explicit choice |
| Files | local disk + normal transfer pipeline | not in initial private MVP | only specifically selected files, read-only |
| Keys | stored using suitable identity/key storage | per-session secrets cleared on end | provider credentials stored securely; never in prompts/logs |
| Cloud | normal chat service allowed | **not allowed for session payload/relay** | only selected content sent to explicitly chosen remote provider |

Important: ordinary online messaging should target authenticated, encrypted transport and later end-to-end encrypted message content. Do not advertise full E2EE until a concrete protocol, key lifecycle and verification exist.

## M2 private transport

Each signed-in desktop generates an in-memory key and obtains a short-lived client and server certificate over the authenticated account-service connection. It advertises its private UDP endpoint via mDNS and a server-scoped account tag. An optional company discovery domain is queried through DNS-SD for approved routed subnets. Discovery records never grant access to a chat.

The nearby list reads the existing mDNS cache and configured site DNS-SD records. It filters addresses through the LAN policy and groups addresses by service instance. The renderer gets eligible instances even with an empty conversation directory. Shared conversation membership supplies tentative account matches. Graceful departures remove entries through mDNS. After a lost connection, cached entries can remain until their discovery records expire.

The host device coordinates a temporary room over mutually authenticated QUIC streams. Named room scope identifies the source conversation and exact invited accounts, and the receiver checks that scope against its cached directory. The first device decision for an account is applied by the host. The accepting device signs the session scope. Other devices verify that approval before they enter automatically. A QR join presents an expiring capability through the authenticated stream. The host sends participant metadata and new text to currently joined devices, with no historical replay.

A nearby invitation uses the same consent exchange and stays on the selected connection. The initiator connects to an advertised address allowed by LAN policy, verifies the account-service certificate, and compares the authenticated account to the discovery tag. A shared conversation uses the existing room scope. Otherwise the scope contains exactly the two accounts and no conversation ID. The latter path requires the nearby certificate name on the target and support for standalone invitations on both clients.

Ordinary private transport filters both IPv4 source and destination to a directly connected subnet and applies TTL one. An explicitly configured company site binds to an approved local interface and allows direct UDP among the specified private prefixes. The private runtime owns no M1 storage handle. The desktop rebuilds discovery and closes existing private connections when the eligible adapter set changes, including a switch between adapters with the same address. Invitation windows check the Windows notification state independently of certificate renewal.

```text
Existing chat ──[ghost]──> Named invitation
Private QR ──[valid scan]──> Joining
Named invitation ──[verified account accepts]──> Joined on eligible devices
Joined ──[ghost]────────> Switch modes in the source conversation
Named participant ends ──> Clear local room and return to normal chat
```

Private data does **not** travel on the normal channel during discovery, failure, reconnection or cleanup. Explicit confirmation is required for a new private session.

For a named room, the ghost button returns to the same normal conversation while the LAN lobby holds its private feed in memory. The unread dot reflects messages from other participants after the last visible, focused view. The Windows invitation window and the main view share one invitation decision. A repeated acceptance cannot send the decision or reopen the main view a second time. A closed direct room drops its transcript, unread state, drafts and connection state.

## M3 assistant requests

The Windows composer opens an AI draft in the active normal conversation. The native command checks the signed-in account's normal conversation directory, queries bounded SQLite text, and reads only the individual files selected via the native picker. It freezes the resulting prompt, messages and file text into an in-memory snapshot for review. A one-use confirmation ID releases that snapshot to the selected provider adapter over HTTPS. Closing the normal composer, accepting a private invitation or signing out clears pending authorization.

The Responses adapter handles OpenAI, DeepSeek and Grok at their default HTTPS endpoints or an explicitly configured HTTPS base URL. Model discovery uses a separate authenticated list request without chat context. Windows DPAPI protects keys in the per-account profile. Unshared replies live in the signed-in desktop workspace. An explicit share publishes a provider-tagged message and its recipient list in one PostgreSQL transaction. The timeline uses the reply's generation time, bounded by the conversation and the server's clock. The sync feed uses publication sequence. The sync feed filters the reply by its audience. Conversation previews use the recipient’s local history.

## Boundaries worth keeping strict

1. The private-session transport API has **no relay endpoint** and no permission to write a transcript to the persistent chat store.
2. AI context extraction is a separate user-confirmed operation; it has **read-only capabilities**, no arbitrary filesystem handle and no chat database write handle.
3. File streaming should be chunked/resumable with transfer identity and integrity verification; do not overbuild distributed storage or deduplication.
4. Only introduce offline queues, migrations, reconnection complexity and cross-device syncing when the current module's user journeys require them.

## Design trade-offs pending implementation

- Choice and audit of an established E2EE protocol for online messages.
- Mobile background constraints and platform-specific discovery.
- Optional on-device AI models and their privacy review.

Record decisions in [DECISIONS.md](DECISIONS.md) when one is actually made.

## M1 Local-First RFC

**状态 · [Issue #46](https://github.com/openUrain/Zibblu/issues/46)，Windows 实施。** 数据归属依据 [D-018](DECISIONS.md#d-018--m1-local-first-与临时文件投递)。本节描述 v2 和隔离验收路径。对外部署及自动回收由 Owner 单独批准。

### 数据归属与期限

| 数据 | 数据源及期限 |
| --- | --- |
| 聊天历史、回显、未读、搜索、引用和已保存的附件 | 按服务、账号和安装实例隔离的 SQLite 与应用管理文件。正常会话离线期间可读本机历史及排队发送。重启后验证登录再打开资料。 |
| 待投递文字和显式共享的 AI 文本 | 自发布起默认 72 小时。重试沿用原截止点。到期从在线路径返回无正文占位，开启回收后分批清除正文。SQLite 中原有正文继续可用。 |
| 成品文件、未发布上传、设备活动 | 期限、设备上限和清理间隔使用部署参数。成品文件 7 天、未发布上传 24 小时、设备活跃 30 天、设备上限 5 仍是候选值。新建隔离库时可用短 TTL。文件内容 ACK 不缩短可下载期限。 |
| 服务端无内容记录 | 账号、目录权限、发布键、顺序、受众、引用、撤回和设备任务结果。文本或文件过期后保留轻量消息头，待确认事件在该活跃设备 ACK 或退役前保持可扫描。 |

应用的数据库和受管文件是长期副本。无其它本地副本时，设备损坏可能使已过期的历史无法恢复。

### 发布、设备与权限

1. **先入 Outbox。** SQLite 在同一事务保存稳定 nonce、会话、内容或待分享受众、创建时间和发送状态。附件用持久化传输任务和受管暂存文件，原上传 UUID 同时作为发布键。网络或临时服务错误退避重试；明确拒绝保存在本地，用户可以按原键重试。服务端发布响应和自己的同步回送都按同一键合并。正式副本、确认结果与删除出站任务同事务提交。
2. **服务端原子发布。** 发布、入群、设备登记、撤回和同步检查点使用同一 PostgreSQL feed 锁。发布分配全局 `seq`，写入账号受众、各活跃设备的任务及已提交水位。相同发送者和 nonce 或附件 UUID 返回原序号、时间和期限。会话、类型、引用及 AI 受众不一致时拒绝，仍暂存的载荷也校验一致性。
3. **设备登记与退役。** Windows 持久化随机安装凭据后发起密码登录。服务保存摘要并颁发设备 ID，把会话绑定到账号、设备及 feed epoch。登录续签需同一个安装凭据。设置页可退役本账号其他设备。退役、账号撤销、配置的活跃期到期或 epoch 变更都会阻断旧会话。每次正常页 ACK（含空页）更新活跃时间。
4. **多设备。** 每台活跃设备独立接收已分配消息。首次登记从当时水位开始；账号仍未确认且未到期的积压会分给新设备，起点提前到第一条积压之前。附件只有元数据 ACK 时，新设备仍可接续内容任务。已完成的云端投递不用于自动历史漫游。保留原本机历史的设备重新登记后，用 `/v2/history/status` 对授权序号批量补齐撤回。
5. **受众。** 消息头及账号受众在发布时固定。同步、引用、附件访问和 ACK 均按当前账号、成员状态以及 `joined_after` 复核。群退出及重入期间的旧线上序号无法重新访问。AI 只向明确选择的受众及发布者发事件。联系问候先在本机出站队列保存请求 UUID；审批前的问候暂存有期限，批准时有内容才按该键发布。
6. **回执。** 单聊双方开启时，仅当前最末条普通、自发、未撤回的消息可以显示回执。对端任一设备的精确元数据 ACK 产生单勾；实际前台视口显示该消息后，由持有对应落盘证明的设备报告查看，才可产生双勾。偏好变更以提交水位划定新资格。到期占位不产生送达或未读。发送方在本地可看到在线投递窗口结束；关闭回执时仍保留投递所需的内部证据。

### 同步窗口与确认

- `GET /v2/sync?cursor=…` 使用已认证设备的当前确认游标。在事务中读取提交水位 `H`，从 `(acked_after, H]` 中扫描已分配事件。发布和撤回同序列，撤回以 `event_seq` 指向原 `message_seq`。页满时推进至最后扫描项，穷尽时推进至 `H`，包括空页。
- 同一个设备只有一个待确认窗口，随机 `page_id` 固定。重取时按当前权限与服务端时钟投影原窗口。失权事件省略，过期或撤回降级为占位。不给其他设备或未知游标提供页面。
- 普通文字上限为 64 KiB UTF-8。新窗口每页最多扫描 32 个事件，为 JSON 转义和元数据保留空间，兼容已有客户端的 16 MiB 响应上限。此前已发出的未确认窗口按原页继续。
- 客户端在一个 SQLite 事务写入本页消息、撤回、目录及待发 ACK。撤回墓碑先于迟到的发布响应，已保存的正文及附件描述不会被 TTL 占位覆盖。失败则仍可从原游标重取。
- `POST /v2/sync/ack` 包含 `page_id` 与实际落盘的 `metadata_received` 序号。服务端校验窗口、名单与有效性，再记设备与账号结果。已过期载荷只确认窗口。客户端持久化服务端返回的 `cursor` 后继续下一页，丢失 ACK 或响应时补发已保存的 ACK。最近一页重复确认返回原结果。

### 文件内容确认

- 上传块和字节偏移同事务提交。最后验证长度和 SHA-256，发布消息、确定成品期限并分发任务。发布响应丢失后，可以凭原 UUID 查询和完成同一次上传。
- 元数据 ACK 只确认卡片。分块、预览和范围读取均复核当前权限。已授权且成品过期返回 `410`。未保存的卡片提示文件过期并可联系发送者重发；已保存的原文件继续在本机可用。
- `POST /v2/files/{id}/download` 为显式下载建立设备内容任务。安全保存目标并校验 SHA-256 后，客户端把本地文件清单和待发 ACK 同传输结果提交。`POST /v2/files/{id}/ack` 表示 `content_downloaded_and_sha256_verified`，提交摘要并复核设备、权限、成品和有效期。续传半途、预览、摘要错误和目标冲突都不计入内容 ACK。
- 文件完整副本可从受管目录再次保存，无需请求服务器块。取消、到期和撤回处理在线与应用管理的入口，用户另存的副本由用户保管。

### 视频衍生资源

原片完整校验后按原协议发布；对具有 MP4/MOV、Matroska/WebM 或 AVI 容器特征的普通文件，在同一事务登记待处理的封面和预览。独立后台工作器以数据库事务 advisory lock 限制同一库的并发为一，读取原片分块到有界临时目录，以 ffprobe 检测真实视频流，再生成 JPEG 封面和 H.264/AAC MP4 预览。转码期间不持有消息 feed 锁；提交衍生资源前取得 feed 锁复核源附件与消息期限、撤回及配额。源过期或已撤回则不保留迟到结果。进程中断回滚待处理事务，重新启动可继续处理仍有效的任务。

`video_resources` 以原附件 UUID 与 `cover` / `preview` 作为独立资源身份，`video_resource_chunks` 保存独立分块。每项有自己的状态、MIME（JPEG/MP4）、字节数和 SHA-256；失败项没有字节数和摘要。只有成品衍生资源占用持久化字节配额，现有单账号 2 GiB、服务 16 GiB 和原附件数量上限不变。衍生资源通过外键随原附件删除；其读取不建立原片内容 ACK。

- `GET /v2/files/{id}/media`：Bearer 鉴权，返回 `attachment_id` 与 `cover`、`preview` 两项。每项含 `id`（原 UUID/类型）、`status`、`mime`、`size`（字节或 null）、`sha256`（独立摘要或 null）。状态为 `pending`、`ready`、`unsupported`、`failed`、`not_smaller`、`quota`。没有衍生资源记录的历史附件或非视频为 `unsupported`。
- `GET /v2/files/{id}/media/{kind}`：Bearer 鉴权读取对应资源；`HEAD` 返回完整资源的 MIME、Content-Length 与 Accept-Ranges。GET 支持单一 bytes 范围、后缀范围与无结束位置范围，成功为 206 并带 Content-Range，非法或不支持的多范围为 416 并带总长度；无 Range 为 200。每次从数据库读取最多一个原生分块并重新校验授权、撤回及期限。
- 无认证 401；非可见、已撤回或原件不存在 404；已授权但到期 410；处理中 409；生成失败 422；不支持或预览不比原片小 415；配额不足 507。不能通过衍生资源路径获取服务器任意文件。所有响应私有且禁止缓存。
- 不改变现有发布、同步、上传、原片下载或内容 ACK 的请求及响应。旧客户端继续访问原片；新客户端可单独获取衍生资源，无需先下载原片。

### 存储、回收与回滚

| 组件 | 用途 |
| --- | --- |
| `m1_feed`、`m1_devices`、`sessions` | 提交水位、epoch、每设备游标与未确认窗口、账号和设备绑定 |
| `messages`、`m1_recipients`、`m1_device_events` | 轻量消息头、限时正文、受众与精确投递证明、独立文件内容状态 |
| `attachments`、`attachment_chunks`、`m1_expired_uploads` | 限时描述与分块，以及短期未发布上传终止标记 |
| SQLite `inbox`、`outbox`、`sync_checkpoint`、`recall_tombstones`、`local_read_positions`、`local_files` | 长期历史、待发送与确认、每设备未读和已核验文件 |

- 可选清理任务在有界事务中回收过期文字、问候与上传分块，清理到期设备和已终结任务。读取始终先判定期限。事务失败整批回滚，下次轮询重试。
- WAL、快照和备份有各自的保留期限；应用侧到期只说明在线访问已结束。加密和限权的恢复归档、7 天备份候选期限、介质与复制滞留核验属于上线前的运维决策。[安全边界](SECURITY.md)
- `0007` 只允许在没有旧账号的 Zibblu 数据库应用。旧数据和旧客户端保留在旧环境。Windows 使用新资料结构；签入 v2 后历史不会自动导入。当前交付提供新资料目录初始化和通过授权重新登记，手动备份包导入、跨设备历史迁移及生产 PITR 切换演练仍待后续工作。[隔离初始化及回滚](ONLINE_DEPLOYMENT.md)
- 普通消息及附件走 M1。M2 内容只走已认证的局域网连接。M3 提供商只接收逐次确认的本地快照，显式共享回复另行按 M1 处理。

### 本机验收

用隔离 Docker PostgreSQL 配合 Rust 集成测试覆盖稳定 nonce、提交前失败、服务器提交后响应丢失、跨设备游标、空页和分页、退役及积压接续、过期回收事务回滚、撤回与受众、附件权限和双 ACK。两个独立 Windows 资料目录用于 WebView2 实机验收。Dell 与 Win11 物理双机验证独立记录，生产 TTL 需另行启用。
