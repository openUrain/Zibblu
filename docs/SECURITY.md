# Security & privacy boundaries

This document records the security boundaries of M1, M2 and M3.

## Threats and limits

We aim to prevent server routing/storage of private-session payloads and prevent AI from accessing anything outside a request's authorized scope.

We **cannot** promise protection from: screenshots or cameras, recipients manually copying messages, malware on a device, compromised operating systems, hardware-level interception, or all OS swap/crash/notification residues. Avoid wording like “forensically untraceable”.

## Private LAN sessions

- A device generates its private key in memory. The authenticated account service certifies that device's public key and account for at most one hour. Mutual QUIC TLS proves possession and verifies the account. The issuer is trusted for identity and keeps its signing key in the service database. It receives no LAN invitations, membership decisions or private messages.
- Chat-based invitations are restricted to the existing direct conversation or selected members of a shared group. A nearby invitation can also name one account outside the chat directory. The initiating device validates the selected LAN endpoint's account-service certificate and checks the certified account against its discovery tag. For an unknown account, the certificate key must also match the advertised device instance. The recipient sees the certified host identity and must explicitly accept on that connection. The first valid response is authoritative. A device signs its approval over the exact room scope and host device.
- A private QR room is open to signed-in LAN holders of its bearer code for 15 minutes. A photograph or forwarded copy grants the same access within that window. A new device receives only messages sent after joining.
- The default UDP policy limits peers to a directly connected RFC 1918 or IPv4 link-local subnet and sets the outgoing TTL to one. A managed site can opt in to explicitly approved private IPv4 prefixes and internal DNS-SD discovery. Network administration controls routing and firewall scope for those prefixes. Private IPv4 addresses alone cannot prove physical proximity. Local observers can see discovery and traffic metadata.
- iPhone uses system Bonjour for the declared Zibblu service and binds IPv4 QUIC to the current Wi-Fi interface. Discovery requires Local Network access. Backgrounding or changing the active Wi-Fi path closes the private lobby and removes its transcript and drafts from app-controlled state. The app covers its window while inactive to limit app-switcher previews.
- No cloud relay, no NAT traversal via Zibblu servers, and no fallback to ordinary online chat.
- Content must not be sent to telemetry, logs, crash reports, normal-message storage, search indices, notifications or cloud AI.
- Private text is kept in bounded memory during the session. End, disconnect, timeout and sign-out discard local private state. Switching conversations leaves the active background session running. A restart opens no private transcript.
- Dismissing a disconnected room keeps only its ID in memory for the current sign-in, including after network changes. The client refuses automatic re-entry to that room. An explicit scan of a valid private QR code expresses fresh consent.
- Connection UI must distinguish `connecting`, `verified private`, and `disconnected`. Never badge a session “private” based only on an SSID or IP range.
- Block accidental exports, copy-to-normal-history and private-content previews in the first release.

## AI least privilege

- `@assistant` is a trigger, not blanket consent. Present a scope selector and explicit provider disclosure on every invocation.
- The client prepares an **immutable scoped snapshot** of messages and exact file objects. AI connectors receive neither DB query access nor arbitrary filesystem access.
- No modify/delete APIs, shell/tool execution, silent posting, autonomous actions or ongoing background collection.
- Use read-only file handles for selected attachments and guard against path traversal, symlinks and scope expansion at the capability boundary.
- Treat message/file contents as untrusted data. Instructions embedded in them cannot expand AI permissions.
- Private-session content is not sent to cloud AI. If on-device AI is later introduced, review its actual network behavior before allowing private context.
- The Windows client encrypts provider keys with user-scoped DPAPI and binds each key to its provider. The renderer receives configuration metadata, including the API base URL. Changing the base URL requires entering the key for that endpoint.
- iPhone keeps provider keys, the saved sign-in and installation credentials in device-only Keychain items accessible while the device is unlocked. Assistant settings store a provider-bound reference. The document picker grants access to a selected sandbox copy, which is read as a bounded UTF-8 snapshot for the current AI request.
- The native app verifies normal-chat membership, reads only the selected text and snapshots UTF-8 files from a native picker through read-only handles. Reparse points, nonlocal path forms and files larger than 32 KiB are rejected. The provider receives only the prompt, selected text with sender handles, and picked filenames with their text.
- The send action consumes one in-memory snapshot of up to 60 text messages and 256 KiB. The disclosed HTTPS endpoint receives one request with normal certificate validation, including OS-trusted roots for managed gateways. Redirects and retries are disabled. OpenAI and xAI requests set `store: false`. DeepSeek's Responses API is stateless. Provider-side handling remains subject to the provider's terms.
- Unshared replies stay in memory for the current sign-in. When the user explicitly shares a reply, the server validates the selected chat members and filters temporary reply delivery by that audience. Conversation previews use locally held history.

## Ordinary chats and files

临时投递、设备游标与回收见 [M1 Local-First RFC](ARCHITECTURE.md#m1-local-first-rfc)。

- Authenticate users and secure transport; design eventual message E2EE with a standard, reviewed approach rather than inventing cryptography.
- Bound file sizes/resources, validate transfers, apply integrity checks and avoid overwriting unrelated destination files.
- Clearly distinguish privacy expectations of normal server-assisted traffic and LAN-only temporary traffic.

M1 使用 Argon2id 保存密码摘要。服务端保存七天会话令牌和安装设备凭据的摘要；会话绑定账号、有效设备和当前 feed epoch。Windows 用用户范围 DPAPI 保存登录令牌和按规范化服务地址及账号隔离的安装凭据。WebView 只收到账号卡片字段。恢复后先向原服务校验身份和设备，再打开本地资料。退出清除保存的登录令牌并尝试撤销在线会话；退役会撤销该设备的会话和投递任务。

远程客户端要求 HTTPS 和正常证书验证。环回开发可用 HTTP。在线服务在有效期内可读 M1 载荷，普通聊天尚无端到端加密。SQLite、暂存和已核验本地附件使用操作系统账号的文件权限。

每次附件分块、预览和内容 ACK 检查当前会话、账号、成员、历史截止点和撤回状态。上传修改由所有者发起并复核发送权限。文件或目录归档上限 256 MiB。服务按账号预留最多 2 GiB 和 256 个传输，全服务上限 16 GiB 和 4,096 个传输。超过配置的期限后停止服务对应载荷；可选自动清理释放数据库空间与上传配额。默认关闭自动清理，生产期限和开关须单独审批。

SQLite 落盘后才发送元数据 ACK。完整保存并校验长度和 SHA-256 后，设备才提交独立的文件内容 ACK。未授权请求统一按不可见处理，授权用户可收到过期回应。内容响应使用 `Cache-Control: private, no-store`。服务端清理只结束数据的在线可用性，WAL、数据库备份、快照和代理需执行各自的留存与访问控制。

Local attachment paths come from a native picker or an explicitly confirmed drop. Pasted images supply bounded image bytes, without a path. Media previews use the same authenticated chunk access as downloads. Image previews check SHA-256 and serve a content type derived from the validated bytes with `nosniff`. Image decoding is limited by input size, dimensions and decoded pixels. Animations are limited to 240 frames and 64 million canvas pixels in total. The app does not extract received archives or overwrite existing destinations.

Contact approval can only be performed by the recipient. Recall can only be performed by the original sender within five minutes according to server time. Recalled attachments are unavailable through the service, and the app keeps a notice in the timeline. Copies already saved by a recipient remain on that device.

A contact greeting is visible in the two accounts' request lists during its delivery window. Approval publishes a still-valid greeting once, using the request key in the transaction that establishes the contact relationship. When someone explicitly opens a LAN participant's profile or requests contact, the normal service receives the account ID and any greeting the user enters in that separate form.

Signed-in users can view account handles and avatars. A profile's cover and bio require the owner's account or an approved contact relationship. Contact remarks and details belong to the viewer's address book and are returned only to that viewer. After confirmation, the desktop decodes and normalizes supported profile images. The service accepts decoded, static PNG data within 2 MiB and 1024 × 1024 pixels for the authenticated account.

服务端复核每个窗口实际提供的序号及当前权限。游标与页绑定到认证的设备。设备间的同步位置彼此独立，前台查看有本设备的精确落盘证明才推进单聊回执。双方开启时，发送者只能查询当前末条自发普通单聊的状态；偏好切换后的资格从提交水位重新计算。未读计数和预览在 SQLite 按可见历史计算。

Delete for me hides the message in the current SQLite profile, including search, quotes, file history and AI selection. An incomplete transfer remains manageable in Transfers. Other devices and other accounts retain their own views. Quotes reference a message in the same conversation and resolve against each viewer's authorized history. Normal drafts live in the signed-in renderer's memory. Private drafts also clear on session termination or disconnect.

Normal group join codes are renewable bearer invitations available to group members. The server stores a hash and expires each code after seven days. QR admission records a message sequence cutoff, enforced on synchronized messages, previews and file downloads. Existing group members retain the history they could already access.

## Minimal security acceptance at module completion

**01 Chat & Files**
- Authenticated normal message path works; interrupted file transfer resumes and integrity verifies; wrong/unauthorized file request is rejected.

**02 Private Chat**
- Two devices on a reachable LAN establish an authenticated session and exchange a message.
- Peer decline, no LAN, or disconnection never invokes internet fallback or normal-history persistence.
- End/restart shows no retained private transcript. Inspect limited relevant storage/log/network surfaces.

**03 AI**
- Selected message/file content reaches only the chosen provider for one request.
- Out-of-scope messages/files and private-session content do not reach cloud AI.
- AI cannot mutate files or chat history, including via instructions inside the selected content.

Run these **focused** checks after implementing the respective complete module; do not turn them into an unfocused test campaign. Record observed results and unresolved limitations in the PR.
