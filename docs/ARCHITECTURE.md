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

M1 的下一阶段见 [Local-First 协议草案](#m1-local-first-rfc)。

- A single Axum service serves normal-chat HTTP requests. It binds to loopback by default. Docker Compose explicitly enables `0.0.0.0:7841` inside the container and publishes only to host loopback. Remote clients use an HTTPS reverse proxy. Tauri commands use the Rust client for HTTP, SQLite, and local file access. [Online deployment](ONLINE_DEPLOYMENT.md)
- Accounts belong to one service. The original username is stored with case-insensitive lookup and uniqueness. Contact requests can start from a username or contact QR. Approval adds both address-book entries in one transaction. Removing a contact removes both entries and preserves chat history. New direct messages and attachment publication require an approved relationship.
- Windows saves one user-scoped DPAPI record per desktop data directory with the last successful account, canonical server, bearer token and optional avatar thumbnail. Tauri supplies only the card fields to the startup renderer. After the user selects the card, the native client uses the existing authenticated profile route to confirm the saved account is the token owner before opening its SQLite profile. Sign-out removes the saved record before attempting to revoke the server token. An authenticated sync rejected with HTTP 401 closes the local session and returns to sign-in.
- PostgreSQL stores the normal-chat feed and retains messages while recipients are offline. Clients poll for changes and commit batches into per-server, per-account SQLite profiles. The same feed supplies chat history and attachment metadata. A client must be signed in to use its profile. Sending a new message requires reaching the service.
- An upload holds a stable ID, expected size and SHA-256. PostgreSQL commits each chunk with its offset and publishes the attachment message only after verification. Downloads resume from a local partial file and verify SHA-256 before publishing to the selected destination.
- Recall uses the same ordered server sequence as publication and a separate cursor for message updates. The client replaces affected SQLite payloads with recall notices and removes those attachments from file history. This preserves the existing message cursor and history.
- The native WebView resource handler requests authenticated file chunks for previews. Video uses ranges. Images use an extension as a preview candidate, then require a matching SHA-256 and successful bounded decoding. The decoded format determines the response MIME type. Preview and download serve the original bytes. Clipboard image bytes are validated before entering the existing staged upload path.
- The desktop converts confirmed avatar and cover uploads into bounded, static PNGs. The profile service still validates the stored PNG and enforces the existing avatar and cover access rules.
- An additive migration stores profile metadata, per-owner contact notes, unread cursors and quote references in PostgreSQL. The unread cursor starts at existing history during upgrade. Account and conversation polling supplies unread counts. SQLite stores per-device hide markers independently from the sync cursor.
- A receipt-capable client requests delivery batches from the existing message feed. The service issues an opaque batch ID for the exact eligible incoming sequence numbers in a page. SQLite commits that ID with the inbox rows before the native client posts the ACK. A failed ACK stays queued and does not block message sync. The service applies each batch once and advances per-member delivery cursors. A signed-in device can retry batches saved by an earlier sign-in to the same account. Pending batches are capped at 64 per issuing session on the service and 64 per local profile. Service batches expire after seven days and are pruned as new batches are issued. This queue is independent of retained message history.
- The active direct-chat view asks for the receipt of its latest eligible outgoing message. The service uses the peer's delivery cursor and foreground-view cursor with mutual opt-in cutoffs. Older clients retain the original sync and read wire shapes. An older service supplies no delivery batch, so new clients omit checkmarks. Migration `0006` adds fields and a pending-batch table. Rolling back application code against a migrated database requires a build carrying the same migration or restoration of an upgrade-time backup.

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
Joined ──[switch views]──> Session continues in background
Host ends or exits ──> Clear room state
```

Private data does **not** travel on the normal channel during discovery, failure, reconnection or cleanup. Explicit confirmation is required for a new private session.

## M3 assistant requests

The Windows composer opens an AI draft in the active normal conversation. The native command checks the signed-in account's normal conversation directory, queries bounded SQLite text, and reads only the individual files selected via the native picker. It freezes the resulting prompt, messages and file text into an in-memory snapshot for review. A one-use confirmation ID releases that snapshot to the selected provider adapter over HTTPS. Closing the normal composer, accepting a private invitation or signing out clears pending authorization.

The Responses adapter handles OpenAI, DeepSeek and Grok at their default HTTPS endpoints or an explicitly configured HTTPS base URL. Model discovery uses a separate authenticated list request without chat context. Windows DPAPI protects keys in the per-account profile. Unshared replies live in the signed-in desktop workspace. An explicit share publishes a provider-tagged message and its recipient list in one PostgreSQL transaction. The timeline uses the reply's generation time, bounded by the conversation and the server's clock. The sync feed uses publication sequence. The sync feed and conversation previews both filter the reply by its audience before returning data to clients.

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

**状态 · Issue #41，M0 设计评审。** 数据归属和临时文件投递采用 [D-018](DECISIONS.md#d-018--m1-local-first-与临时文件投递)。本节描述待实现协议。上文记录当前行为。生产清理须由 Owner 单独批准。

### 当前仓库审计

审计基线为 `b396030`。Issue #39 已关闭，PR #40、#42、#43 和 #44 的 Windows 改动已进入 `main`。

| 路径 | 现状和迁移依赖 |
| --- | --- |
| [Rust 客户端](../crates/core/src/client.rs#L255) | 按规范化服务地址和账号隔离 SQLite。`/v1/sync` 用 `MAX(inbox.seq)` 及单独的撤回游标。`send_message`、`publish_ai_reply` 先请求服务，自己的消息靠回拉入库。移植到临时队列时，仅收到 HTTP 确认就清理服务器副本会丢失本机历史。 |
| [在线同步](../services/online/src/chats.rs#L53) | `messages` 同时提供正文和目录的 `preview`、`latest_seq`、`unread`、`read_through` 校验、引用、撤回及 AI 分享可见性。`lock_message_feed` 已串行化发布、撤回和入群截止点。 |
| [送达及已读](../services/online/src/chats.rs#L265) | `0005` 引入 `message_receipts`，`0006` 已改由 `pending_delivery_batches` 和账号级 `members.delivered_through/viewed_through` 支持最后一条回执。批次在 SQLite 提交后 ACK，但账号级最大序号无法充当设备级清理证明。保留两版兼容期内的现有行为。 |
| [附件](../services/online/src/files.rs#L182) | `0001` 用 BYTEA 分块，完成时校验 SHA-256 并发布 `messages.attachment_id`；下载经该发布行和成员截止点授权。现有文件只有手动取消未发布上传和撤回访问控制，缺少成品 TTL 及完整下载 ACK。 |
| [其他发布入口](../services/online/src/accounts.rs#L277) | 联系人批准会把问候写入消息流；附件完成和 [AI 分享](../services/online/src/chats.rs#L664) 也会发布。AI 共享名单由 `ai_reply_readers` 关联消息序号，预览、同步和引用须继续按该名单过滤。 |

### 数据归属与期限

| 数据 | 归属与拟定期限 |
| --- | --- |
| 正文、已收元数据和已保存附件 | 本地 SQLite 和应用管理的文件目录为长期真源。历史、搜索、预览、引用、撤回状态按本地已持有记录渲染。缓存清理与删除本地历史可分别执行。无备份的设备损坏可能使历史永久丢失。 |
| 待投递文字和明确分享的 AI 文本 | 从发布事务记录的服务端 `published_at` 起 **72 小时**。只有全部仍有效的设备任务都确认，且每个仍获授权的目标接收账号至少一台设备已落盘时，才可提前回收正文。服务器时钟判定到期，重试不延长期限。尚未入队的 outbox 由设备保管。 |
| 已发布图片、视频、文件与目录归档 | 建议从发布起保留 **7 天**。沿用 PostgreSQL 分块，元数据和字节在期限内按授权可取，下载 ACK 不缩短期限。撤回或权限撤销立即停止相关在线访问。 |
| 未发布上传 | 建议从服务端创建起 **24 小时**，期满释放分块与配额。中断恢复用原上传 ID，旧上传过期时由发送者重新发起。 |
| 目录和协议元数据 | 服务端保存账号、联系人、设备、群和成员权限。会话存续期间保留无正文的消息标识、排序、撤回墓碑、可见性及必要的幂等结果，设备任务与成员下载明细按下文回收。 |

正文过期只影响未完成的在线投递。接收端尚未收到时可得到无正文的过期占位，发送端可看到到期结果。文件在线副本不可用且本机没有已验证副本时，显示「文件已过期，可请对方重新发送」。保留本机已收到的消息元数据，已验证文件继续从本地打开。7 天和 24 小时是 Zibblu 的候选值，连同设备和备份期限在生产启用前由 Owner 确认。

### 发布、设备与权限

1. **发送。** 先在 SQLite 提交 outbox，保存稳定 UUID、会话、类型、不可变正文或已暂存附件及引用目标。联网后提交 `nonce`，附件沿用上传 UUID。服务端在同一事务中鉴权、分配序号、发布、建立账号与设备任务、推进提交水位，并用服务端发布时间计算到期。`(sender_id, nonce)` 和附件 UUID 唯一。相同键重试返回原 `seq`、发布时间、期限和结果。会话或类型不符返回冲突。暂存期内也校验内容、文件描述或 AI 受众是否一致，清理后凭不可变键返回原结果。设备以该键合并发布响应和自己的同步回送，在同一 SQLite 事务写定正式序号再结束 outbox。重启可继续原请求。明确重发才分配新键。
2. **送达。** `outbox → queued → delivered / expired`。`queued` 表示服务端接单。每个接收账号至少一台有资格设备确实完成本地提交才算该账号已送达，其他设备继续独立投递。群发在全部目标接收账号确认后才整体算送达，有账号过期则可表明部分过期，退群失权单列为终止。发送账号的其它设备也有同步任务，自己的落盘不计入对端送达。原发送设备用 outbox 和发布键恢复。
3. **登记及退役。** 登录账号明确登记安装实例，服务端颁发设备 ID 和不透明凭据，凭据哈希留在服务端，原始凭据使用系统安全存储。会话须绑定已登记设备，换 token 需证明拥有原设备凭据。按账号限制活跃设备数，候选上限为 5 台。每次成功同步确认（包括空页）延长活跃期，建议 **30 天**。失活或主动退役时撤销凭据并关闭该设备的未清任务。其它设备的任务继续，账号尚未完成的投递资格保留至截止点。退役设备或被替换的数据库以新设备登记，不能沿用旧凭据和游标。
4. **受众及入群。** 发消息、AI 分享和文件发布复用现有普通聊天权限。发布时从会话成员和明确的 AI 分享名单确定受众，读取、续传、ACK、引用校验时重查当前成员权限及 `message_seq > joined_after`。发布与入群使用同一 feed 锁，入群截止点取已提交水位。新入群或重新入群者只获新截止点之后的授权。成员退出撤销在线访问，已保存的本地副本由用户设备管理。
5. **离线及新设备。** 发布时给每台活跃设备单独建任务，离线设备包含在内。登记与发布串行，登记时锁定受众任务并与确认、清理串行，给新设备分配本账号仍有权限且未到期的未完成事件。文字以该账号无落盘 ACK 为准，附件在该账号无内容 ACK 时也分配其文件卡片与内容任务。已有设备上的待确认任务继续独立投递。新设备从登记水位起步，有积压时在发出首个游标前将服务端起点设到最早分配事件之前。这样旧设备之后退役，新设备仍有有效任务可接续。已完成的历史由用户确认后从本地备份或原设备导入。
6. **联系人问候。** 联系人请求仍是控制数据，其可选问候正文单独暂存 72 小时。发送端先本地保存带稳定请求 UUID 的问候；批准时沿用该键幂等发布仍有效的问候，过期问候不进入聊天，联系人请求仍可批准。

### 同步窗口与确认

v2 继续使用服务器分配的全局 `seq`。发布和撤回共用序号，撤回以自己的 `event_seq` 指向原 `message_seq`。复用 `lock_message_feed` 保证已提交事件按序变得可见，水位 `committed_through` 与事件及分发任务在同一事务提交。排序显示仍用 `(created_at, seq)`，AI 分享可按生成时间展示，TTL 从发布时间计算。

1. **取页。** `GET /v2/sync?cursor=…` 在设备会话下解析当前确认游标为服务端位置 `after`，在一致性快照中读提交水位 `H`。按 `(device_id, event_seq)` 查询该设备在 `(after, H]` 的事件任务，排除仅登记文件内容下载的行，取页大小加一条。还有任务时以 `has_more` 告知客户端，内部 `next_after` 取本页最后扫描的序号。穷尽区间时内部 `next_after = H`，空页也适用。过期任务返回授权范围内的无内容墓碑。权限撤销的任务只占扫描位置。其它会话、非受众事件、事务回滚的序号洞无对应任务。本页的发布和撤回按 `event_seq` 合并有序返回。
2. **固定窗口。** 每设备只保留一个未确认的随机 `page_id`，绑定设备、feed epoch、原游标、内部 `next_after` 及该页可 ACK 的序号集合。跨页只传该设备的不透明 `cursor`，全局水位和扫描位置只留在服务端。每个新页分配独立 ID，包括空页及扫描位置未变的页。服务端只接受该设备当前已确认的游标开页。重复获取同页保留窗口及 ID，重新鉴权，已过期或撤回的内容降为墓碑，失权内容省略。重发的内容集合不会增大。客户端提交了旧版页面时可按原 ID 重试，服务器只给有效任务入账。
3. **先落盘。** SQLite 一个事务按原消息序号幂等写入，按撤回事件去正文、索引和受管缓存引用，同时保存已处理页及待发 ACK。过期墓碑不覆盖已存的消息。晚到的发布回送不能复活已经应用的撤回。事务失败时从上一个位置重拉，内容按序号和 outbox 键去重。终端无权推断列表外的序号为过期，也无权用 `MAX(inbox.seq)` 更新同步位置。
4. **再确认。** `POST /v2/sync/ack` 带 `page_id` 和实际持久化的 `metadata_received` 序号。服务端锁设备与任务，确认调用身份、窗口和名单，要求窗口内每条仍可投递的发布都在 `metadata_received` 中，然后记下设备结果、推进内部位置并将 `page_id` 作为新的已确认 `cursor` 返回。撤回事件和过期墓碑只确认已处理窗口，在途过期不补写送达，已落盘内容保留。最近页的重复 ACK 返回同一游标，旧或未知 ACK 不能推进。客户端持久化确认游标后用它取下一页。超时或重启先重发已落盘的 ACK，服务端保留最近页的确认结果以应对响应丢失。
5. **清理边界。** 对仍活跃的设备，已过期或撤回的未确认任务保留轻量占位到设备确认或退役。只清理正文或已终结任务时，不能让该设备跳过尚未确认的窗口。旧设备或 feed epoch 失配返回显式重建提示。已验证恢复的原设备可从服务端已确认的位置重拉，其本地记录仍按序号幂等处理。超过设备活跃期的未投递区间应告知用户。旧资料重新登记时，先以备份恢复的同一流程核验本地序号的撤回和附件状态，再从新基线继续。

### 文件内容确认

- 沿用上传偏移和块大小校验，块与偏移一次提交。发布前校验完整长度及 SHA-256，将成品期限、消息及投递任务一次提交。`complete` 重试按原附件 UUID 返回同一发布。已有分块与暂停记录可以续传。
- `metadata_received` 确认本机附件卡片信息已持久化。每次分块、预览和范围读取都按发布的消息标识、当前成员和加入截止点重新鉴权，并检查已发布、未撤回和未到期。无权请求统一按不可见处理。已授权但成品过期返回 `410`，设备丢弃失效的续传任务并给出重发入口。
- 接收端完整落盘、按声明的长度和 SHA-256 验证并安全发布本地文件后，才持久化并发送 `content_downloaded_and_sha256_verified`。确认带附件 UUID 和摘要，由该设备的认证会话提交。服务端用当前授权、设备任务、期限及成品 SHA-256 校验后入账。崩溃后核对应用跟踪的已落盘文件再补发，失败校验或路径冲突不得入账。预览范围请求及未校验的部分文件不算完成。
- 内容状态按设备分别记 `pending / verified / expired / revoked`。相同 ACK 幂等，元数据 ACK、其它设备的 ACK、到期后的迟到 ACK 均不改变本设备的内容状态。账号至少一台设备验证后记为该成员已下载，否则到期记为未下载。其它设备仍可在有效期内下载。通过本地迁移已持有文件卡片的同账号新设备仍按当前成员权限下载，在发起下载时建立该设备的内容任务，独立于消息游标。

### 从永久正文表迁出读取职责

| 既有读取 | Local-First 数据源 |
| --- | --- |
| `preview` / `preview_seq` / `activity_at` | SQLite `inbox` 计算最后一条本机可见记录，排除本机隐藏项，撤回或过期用占位。目录只下发头像、成员、排序所需的时间和序号等权限内元数据。未下载到本机的正文不用于预览。 |
| `latest_seq` / `unread` / `read_through` | 服务端从轻量消息头和受众求最新可见发布序号，预览对应的时间排序也只取可见记录。各设备从 SQLite 统计真实入库、未隐藏且未撤回的收到项和自己的 `read_through`，过期墓碑不计未读。读到的位置与投递游标分别单调推进，不能用同步水位代替查看。 |
| 最新一条回执 | 仅当按 `(created_at, seq)` 最新可见的普通单聊消息由本人发送、本机显示的末条与该消息一致且双方开启回执，才可显示。对端某设备的精确持久化 ACK 产生单勾，对端设备在前台实际展示并报告已确认的对应位置，且成员 `viewed_through` 覆盖该消息时才显示双勾。服务端保留各成员当前末条的精确送达证明和单调查看游标。偏好变更记录提交水位作为新资格界线，过滤历史状态。任一方关闭时，只保留内部投递所需的 ACK；查询不披露回执。收到对方新消息后旧勾消失，历史、撤回、AI 分享及群聊省略勾。 |
| `reply_to` | 服务端校验会话、成员截止点、原消息受众和撤回状态。输出时按每位阅读者再次校验引用目标。SQLite 取本机已有且仍可展示的原文，缺失、隐藏或撤回时显示「引用消息不可用」。引用不复制原消息正文到新载荷。 |
| 撤回和更新游标 | 保留原作者五分钟窗口，发布记录只读时间和原权限即可校验。撤回在同一事务变更原消息头、禁止再下载并追加一个有序的轻量事件。对所有可能收到原文的活跃授权设备排入撤回任务，包括原 ACK 丢失的设备。客户端为尚未到达的原消息也保存墓碑，撤回后的本地预览、搜索、文件入口和 AI 选材据此过滤。用户已另存的文件由其自行管理。 |
| AI 分享与 AI 上下文 | 未分享的回复留在原来的内存路径。显式选择受众后发布普通事件，按 72 小时政策处理正文，`ai_reply_readers` 保留引用和可见性判断需要的账号集合。目录、同步、引用、未读及恢复校验都沿用该限制。M3 继续只向所选提供商发送一次明确授权的本地快照。M2 LAN-only 的连接和正文不进入该协议。 |

### PostgreSQL 与 SQLite 迁移草案

保留现有序号、附件 UUID 及数据库。下面是目标字段及约束，实施时按兼容阶段追加迁移。

| PostgreSQL | 字段、索引与生命周期 |
| --- | --- |
| `m1_feed` 单行 | `epoch, committed_through`。已有 feed 锁保护消息/撤回写入、登记与入群截止点。消费游标取已提交水位，独立于 PG sequence 的当前值。 |
| `m1_devices` | `id PK, account_id FK, credential_hash, last_seen_at, retired_at, acked_after, cursor_id`，以及一台设备的一页证据 `page_id, after, next_after, offered_seqs`。`cursor_id` 保存当前已确认的随机页 ID 或注册时生成的起始 ID。按 `(account_id, retired_at)` 登记查询，活跃设备按 `last_seen_at` 回收。`sessions.device_id` 绑定原账号和设备，沿用会话的登录与撤销。 |
| `m1_recipients` | `(message_seq, account_id) PK`，外键锚定原消息头和账号。存待设备、已落盘、过期或失权状态，以及成品的成员下载汇总。发布时建立，任务终结后折叠为发布结果，只保留待恢复需要的粗粒度结果。AI 受众长期校验继续用既有 `ai_reply_readers`。 |
| `m1_device_events` | `(device_id, event_seq) PK` 支持顺序分页；`message_seq FK` 指向原消息，`event_seq = message_seq` 表示发布，撤回引用另一事件序号。存元数据和文件内容的独立状态、截止点。只有内容任务的设备行不参与消息分页。索引 `(message_seq, device_id)` 供汇总、撤回，按截止点建待清理索引。退役可级联删设备任务。已确认任务在内容终结、发送结果已汇总且末条回执不再需要该设备证明后清理。 |
| `messages` / `members` | 将 `messages` 用作轻量消息头。复用 `seq, nonce, conversation_id, sender_id, created_at, recall_seq, reply_to`，增加 `kind, published_at, expires_at` 和粗粒度发布终态。有效期内沿用可空 `text` 暂存正文。继续使用会话排序、撤回和发送键索引，添加正文到期的部分索引。旧版退场后按 `kind` 约束稳定标识和有效载荷，允许到期正文置空，保留自引用 FK 和 AI 受众 FK。保留 `members.joined_after`，用成员有效状态处理退群/重入并保留外键锚点。`members.viewed_through` 和当前末条精确 ACK 替代旧账号级 `delivered_through` 及历史 `message_receipts`。 |
| `attachments` / `attachment_chunks` | 增加上传、成品各自的截止时间及终态，按可清理截止点索引。退场旧下载路径后移除 `messages.attachment_id` 指向易过期附件的 FK，保留其唯一 UUID 用于鉴权、幂等和过期回应。成品期限到后删除描述和分块，已发布占位继续由消息头解析。清残留未发布上传时，短期留无内容终止标记供重试判定，终止项释放配额。块对暂存行的级联沿用。旧 CHECK/FK 必须在验证新读路径后才调整。 |

如仍需服务旧客户端，保留 `messages` 旧载荷和 `attachments`、`pending_delivery_batches` 及 `0005/0006` 原接口。当前旧批次的账号级 ACK 和旧读标记无法证明具体设备提交，迁移时不转成 v2 送达。已写 SQLite 的普通历史和隐藏状态保留，未读起点从经校验的旧读位置导入且限定于本机持有记录。

SQLite 保留 `inbox(seq PK)`、隐藏、目录和 `transfers`。追加 `outbox(nonce PK, conversation_id, payload_or_staging_ref, state, seq)`，`sync_checkpoint(epoch, device_id, cursor_id, pending_page_ack)`，`recall_tombstones(message_seq PK, event_seq)` 和 `local_read_positions(conversation_id PK, read_through)`。`transfers` 记录有效期、已核验路径、文件状态和待发内容 ACK。文件确认涉及文件系统和 SQLite 两步，重开时只检查受管暂存件或该任务已登记的最终路径，核验后补齐状态，避免覆盖其它文件。设备进度独立于消息主键。旧本机数据库先留一致性备份，新增表和回填在 SQLite 事务内完成。

### 清理、备份与恢复

- **清理事务。** 读取和内容确认以服务端当前时间及权限为准，即使回收 job 延迟，过期内容也不再发出。取得候选索引中的有限批次，按一致顺序锁行，可用 `FOR UPDATE SKIP LOCKED` 跳过其它工作者正在处理的行，锁后重查期限和确认状态。事务内先关闭任务并更新终态，再清正文、描述或分块。遵循发布和撤回的锁序。失败整批回滚，重复执行只处理仍符合条件的行。撤回、过期、重复完成以及同一成员多设备下载并发时，保持发布行引用有效。授权失效与实际删除之间不透出正文。
- **运维信号。** 观察未确认队列最长年龄、影子新旧序号差及过滤差、过期数量、重复/无效 ACK、校验失败、上传配额、每批行数/字节和重试、SQL 锁等待及 WAL 归档积压。以聚合数据排障。代理、应用及数据库错误日志不得记录正文、附件字节、凭据或含正文的 SQL 参数。带内容响应使用 `Cache-Control: private, no-store`，切换前盘点代理/CDN 及旧预览缓存。
- **数据库备份。** PG 行清理只终止在线访问。WAL、复制滞留、旧页、快照、数据库或卷备份可能含已到期正文和文件。候选策略为内容相关备份、快照和可恢复的 WAL 归档加密、限权并滚动保留 **7 天**，明确旧迁移备份最晚销毁时间、记录介质和检查实际归档/复制水位。用受控恢复演练和留存实测确认能披露的期限。PG 的回收和 VACUUM 不构成物理擦除承诺。[PostgreSQL 17 · WAL 与 PITR](https://www.postgresql.org/docs/17/continuous-archiving.html)
- **服务恢复。** 隔离恢复同一一致点的事件、成员、内容和任务，校验消息/撤回高水位、幂等绑定和抽样文件摘要。完整 WAL 恢复须证明覆盖切换前已受理的提交，序列从已提交和已分配序号之后继续。重放到当前时间后先应用已知撤回及到期，重建同步 epoch，再开放服务。设备从服务端确认位置重拉且保留本机历史。无法证明恢复覆盖已受理写入的旧快照只用于隔离核验，不能直接接管写入或重新对外提供历史内容。[现有部署恢复流程](ONLINE_DEPLOYMENT.md)
- **设备备份与换机。** 经用户确认，在按服务器和账号隔离的配置内，用 SQLite 一致性快照与文件清单保存已收历史、隐藏与撤回标记、已验证的受管附件、可继续的出站任务及其暂存件。可用 SQLite Backup API，文件清单记录路径、长度和 SHA-256，并与 SQLite 快照对应。备份应加密及限制本地访问；设备凭据、会话 token 和 M2 内容不随包迁移。用户另存的文件自行备份，下载临时块可重新拉取。导入时鉴权校验原服务器和账号，排除原设备的待发 ACK，在隔离目录验证 `integrity_check`、版本及摘要，再替换或合并；校验失败保留原件。导入内容以同一 `seq` 去重，向服务端分批核验已导入序号的撤回/附件可用状态，仅返回获准的状态。核验后方将导入内容纳入聊天、搜索和 AI 选材；无法核验的旧组内容保留在原备份。恢复设备用新凭据和新投递基线，未决出站使用原发布键对账。无备份的旧正文无法从已清理服务恢复，保留用户可用的其它设备副本。[SQLite Backup API](https://www.sqlite.org/backup.html)

### 分期上线与回滚门槛

1. **添加 ACK 和事件路径。** 以增量 SQLx 迁移部署 v2 注册、事件和 ACK，在 feed 锁下用现存发布与撤回的最大已提交序号初始化水位。所有发布入口事务内写影子任务，兼容 `/v1`。先用合成记录比对现有可见消息、撤回、序号边界和发布幂等，再独立验证设备 ACK。此时保留所有旧载荷，禁用清理。
2. **回填、迁移本地真源。** 在 feed 锁下为升级设备记录快照水位 `H`，发给该设备 `H` 后的新任务。兼容窗口内以可恢复的导入分页同步 `H` 及之前经授权的旧历史；校验群加入截止点、AI 可见性、撤回和已在本地隐藏的内容。服务端在导入期间用设备绑定的不透明凭据标识快照。导入 SQLite、核验记录范围及可恢复备份后，从绑定 `H` 的设备游标接收后续事件。仅升级路径提供旧历史导入。切换目录和显示职责到本地数据，完成权限、下载和恢复演练。
3. **退场旧客户端后先启用文件清理。** 清理前必须关闭依赖旧历史的所有 M1 读写路径并验证旧版得到明确升级提示。盘点并保护存量历史，发布兼容结束和旧附件可下载的宽限期。存量附件按经批准的迁移宽限期限起算，不能从未知上传时间推算或追溯删除。先在隔离环境测试过期，再由 Owner 单独批准生产第一批。文件先移除过期字节和描述；确认不影响卡片、续传、授权、FK 和恢复。
4. **逐步启用文字 TTL。** 在已核验的 v2 部署先针对新发布的小批次启用，验证到期、ACK 与游标指标，再扩大范围。旧正文的批次需单独核对导入或归档/恢复证据。未完成清单、出现授权差异、正常 TTL 内漏投递、回填对比不一致或无法从备份恢复均阻止清理。
5. **回滚。** 首次清理前可关闭 v2 消费与各 TTL 开关，用包含已应用 SQLx 迁移的兼容服务恢复旧路径；保留已写的 SQLite 和服务端增量。切换期间必须先确认没有只写 v2 的未镜像发布。首次清理后可暂停回收和流量扩大，并回退到懂 v2 数据的兼容版本。已清理内容由设备和保留期内的受控备份恢复。旧版接口或直接将迁移前快照接回生产无法补齐切换后的写入。备份只在隔离环境恢复并经核验。

### 最小数据验收计划

后续实现使用合成账号、独立 SQLite 资料目录、隔离 PostgreSQL、可推进的服务时钟。每项记录客户端可见结果、两端事务状态和服务端实际还可取得的数据。

| 场景 | 通过条件 |
| --- | --- |
| 两个账号，接收方两设备；离线、乱序、丢 ACK | A 发消息，B 的手机持久化并 ACK，PC 离线仍保留自己的任务。重复取页、重复 ACK 和晚到旧 ACK 各执行一次。PC 重连到同一条历史且只入库一次，收到任意其它事件不推进到未交付任务之后。账号回执只能用实际对端证据。 |
| 进程退出 | 在发送本地提交后、服务器提交但响应前、接收 SQLite 事务提交前、提交后 ACK 前分别退出并重启。检查 nonce/seq 只有一条发布，本地出站历史存在，未提交页可重放，已提交页可补 ACK。 |
| 分页缺口和到期 | 制造全局序号洞、无权消息、页尾后一条仍待收及活跃设备过期占位。满页推进到最后扫描位置，空页推进至本快照水位，响应只含设备游标。拒绝未知和跨设备游标或 ACK。跨过 72 小时后未送达状态明确，之后的新消息仍可接收。已落盘副本保持可查。 |
| 文件中断、验证和回收 | 两设备分别收元数据，下载到一半断开后按 offset 续传，对比 SHA-256。仅预览/坏块/错摘要不算完成，一端完成不影响另一端。跨成品期限得到明确过期，已有文件可打开；未发布上传释放配额。模拟清理批次失败及重跑，检查无悬挂 FK、越权读取或本地覆盖。 |
| 群和新设备 | 成员 A/B 可下载，非成员、晚入组、退群及重入的旧序号请求不可见。一个人下载或查看不替代其它账号的状态。让 D1 在仍未 ACK 时保持活跃，登记 D2，再退役 D1。D2 从自身起点收到有效积压，既有游标无需回退。附件只有元数据 ACK 时仍可接续下载，已完整接收、过期或撤回的旧记录不自动重放。已退役设备不能确认，换机经确认导入本地历史并验证账号与服务器隔离。 |
| 撤回、引用、AI、回执偏好 | 接收设备离线直至原文回收再同步撤回，乱序或导入旧备份不能恢复被撤回内容。被隐藏、缺失和未授权引用显示受限；非受众的目录、未读和恢复校验不透露 AI 分享。双方开关各测一次，验证最新自发单勾/双勾、对方随后发消息清掉旧勾及群聊无歧义。M2 无线上正文，M3 只有本次授权快照。 |
| 兼容与恢复 | 并行跑旧版 `/v1` 和 v2，影子期旧历史和下载可用且旧 ACK 不参与清理。检查切换时新旧游标交界无遗漏。在隔离库恢复到一致点，重复发布仍拿原序号，撤回/过期正文仍不可取。模拟旧快照缺少已确认提交时阻止开放写入。恢复正确和损坏的 SQLite 备份各一次。 |
