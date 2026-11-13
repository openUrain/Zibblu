# Security & privacy boundaries

This document records the security boundaries of M1, M2 and M3.

## Threats and limits

We aim to prevent server routing/storage of private-session payloads and prevent AI from accessing anything outside a request's authorized scope.

We **cannot** promise protection from: screenshots or cameras, recipients manually copying messages, malware on a device, compromised operating systems, hardware-level interception, or all OS swap/crash/notification residues. Avoid wording like “forensically untraceable”.

## Private LAN sessions

- A device generates its private key in memory. The authenticated account service certifies that device's public key and account for at most one hour. Mutual QUIC TLS proves possession and verifies the account. The issuer is trusted for identity and keeps its signing key in the service database. It receives no LAN invitations, membership decisions or private messages.
- Named invitations are restricted to the existing direct conversation or selected members of a shared group. The first valid response is authoritative for that account. An approval is signed by that account's device and bound to the host device and exact session scope, so a host cannot enroll other devices by asserting consent alone.
- A private QR room is open to signed-in LAN holders of its bearer code for 15 minutes. A photograph or forwarded copy grants the same access within that window. A new device receives only messages sent after joining.
- The default UDP policy limits peers to a directly connected RFC 1918 or IPv4 link-local subnet and sets the outgoing TTL to one. A managed site can opt in to explicitly approved private IPv4 prefixes and internal DNS-SD discovery. Network administration controls routing and firewall scope for those prefixes. Private IPv4 addresses alone cannot prove physical proximity. Local observers can see discovery and traffic metadata.
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
- The native app verifies normal-chat membership, reads only the selected text and snapshots UTF-8 files from a native picker through read-only handles. Reparse points, nonlocal path forms and files larger than 32 KiB are rejected. The provider receives only the prompt, selected text with sender handles, and picked filenames with their text.
- The send action consumes one in-memory snapshot of up to 60 text messages and 256 KiB. The disclosed HTTPS endpoint receives one request with normal certificate validation, including OS-trusted roots for managed gateways. Redirects and retries are disabled. OpenAI and xAI requests set `store: false`. DeepSeek's Responses API is stateless. Provider-side handling remains subject to the provider's terms.
- Unshared replies stay in memory for the current sign-in. When the user explicitly shares a reply, the server validates the selected chat members and enforces that audience for message synchronization and conversation previews.

## Ordinary chats and files

- Authenticate users and secure transport; design eventual message E2EE with a standard, reviewed approach rather than inventing cryptography.
- Bound file sizes/resources, validate transfers, apply integrity checks and avoid overwriting unrelated destination files.
- Clearly distinguish privacy expectations of normal server-assisted traffic and LAN-only temporary traffic.

M1 uses Argon2id password hashes and expiring opaque session tokens. The service stores token hashes and the client holds the token in memory. Remote clients require HTTPS with normal certificate verification. Plain HTTP is available on loopback for local use. The online service can read normal chat messages and attachments. M1 has no end-to-end message encryption, and its local SQLite and staged files use the OS account's file permissions.

Downloads require authenticated conversation membership for each chunk. Upload changes require ownership, and incomplete uploads are unavailable to recipients. A file or folder archive is limited to 256 MiB. The service reserves attachment storage up to 2 GiB and 256 transfers per account, and 16 GiB and 4,096 transfers overall. Unfinished uploads can be discarded. Published attachment retention is indefinite in M1, so an operator must budget for the database and its write-ahead log.

Local attachment paths come from a native picker or an explicitly confirmed drop. Pasted images supply bounded image bytes, without a path. Media previews use the same authenticated chunk access as downloads. The app does not extract received archives or overwrite existing destinations.

Contact approval can only be performed by the recipient. Recall can only be performed by the original sender within five minutes according to server time. Recalled attachments are unavailable through the service, and the app keeps a notice in the timeline. Copies already saved by a recipient remain on that device.

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
