# Security & privacy boundaries

This document records the M1 security boundary and the requirements for later modules.

## Threats and limits

We aim to prevent server routing/storage of private-session payloads and prevent AI from accessing anything outside a request's authorized scope.

We **cannot** promise protection from: screenshots or cameras, recipients manually copying messages, malware on a device, compromised operating systems, hardware-level interception, or all OS swap/crash/notification residues. Avoid wording like “forensically untraceable”.

## Private LAN sessions

- A private session requires **both participants' affirmative action**, contact authentication and a verified encrypted LAN P2P path.
- No cloud relay, no NAT traversal via Zibblu servers, and no fallback to ordinary online chat.
- Content must not be sent to telemetry, logs, crash reports, normal-message storage, search indices, notifications or cloud AI.
- Private text is memory-first and session-scoped. End, disconnect and restart must not reopen its transcript. Clear accessible buffers and session keys on close; account for abnormal exits.
- Connection UI must distinguish `connecting`, `verified private`, and `disconnected`. Never badge a session “private” based only on an SSID or IP range.
- Block accidental exports, copy-to-normal-history and private-content previews in the first release.

## AI least privilege

- `@assistant` is a trigger, not blanket consent. Present a scope selector and explicit provider disclosure on every invocation.
- The client prepares an **immutable scoped snapshot** of messages and exact file objects. AI connectors receive neither DB query access nor arbitrary filesystem access.
- No modify/delete APIs, shell/tool execution, silent posting, autonomous actions or ongoing background collection.
- Use read-only file handles for selected attachments and guard against path traversal, symlinks and scope expansion at the capability boundary.
- Treat message/file contents as untrusted data. Instructions embedded in them cannot expand AI permissions.
- Private-session content is not sent to cloud AI. If on-device AI is later introduced, review its actual network behavior before allowing private context.
- Do not leak provider tokens in storage, logs, crash traces or requests to unrelated providers.

## Ordinary chats and files

- Authenticate users and secure transport; design eventual message E2EE with a standard, reviewed approach rather than inventing cryptography.
- Bound file sizes/resources, validate transfers, apply integrity checks and avoid overwriting unrelated destination files.
- Clearly distinguish privacy expectations of normal server-assisted traffic and LAN-only temporary traffic.

M1 uses Argon2id password hashes and expiring opaque session tokens. The service stores token hashes and the client holds the token in memory. Remote clients require HTTPS with normal certificate verification. Plain HTTP is available on loopback for local use. The online service can read normal chat messages and attachments. M1 has no end-to-end message encryption, and its local SQLite and staged files use the OS account's file permissions.

Downloads require authenticated conversation membership for each chunk. Upload changes require ownership, and incomplete uploads are unavailable to recipients. A file or folder archive is limited to 256 MiB. The service reserves attachment storage up to 2 GiB and 256 transfers per account, and 16 GiB and 4,096 transfers overall. Unfinished uploads can be discarded. Published attachment retention is indefinite in M1, so an operator must budget for the database and its write-ahead log.

Local attachment paths come from a native picker or an explicitly confirmed drop. The app does not extract received archives or overwrite existing destinations.

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
