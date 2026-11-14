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

- A single Axum service serves normal-chat HTTP requests. It binds to loopback by default. Docker Compose explicitly enables `0.0.0.0:7841` inside the container and publishes only to host loopback. Remote clients use an HTTPS reverse proxy. Tauri commands use the Rust client for HTTP, SQLite, and local file access. [Online deployment](ONLINE_DEPLOYMENT.md)
- Accounts belong to one service. The original username is stored with case-insensitive lookup and uniqueness. Contact requests can start from a username or contact QR. Approval adds both address-book entries in one transaction. Removing a contact removes both entries and preserves chat history. New direct messages and attachment publication require an approved relationship.
- PostgreSQL stores the normal-chat feed and retains messages while recipients are offline. Clients poll for changes and commit batches into per-server, per-account SQLite profiles. The same feed supplies chat history and attachment metadata. A client must be signed in to use its profile. Sending a new message requires reaching the service.
- An upload holds a stable ID, expected size and SHA-256. PostgreSQL commits each chunk with its offset and publishes the attachment message only after verification. Downloads resume from a local partial file and verify SHA-256 before publishing to the selected destination.
- Recall uses the same ordered server sequence as publication and a separate cursor for message updates. The client replaces affected SQLite payloads with recall notices and removes those attachments from file history. This preserves the existing message cursor and history.
- The desktop previews ordinary attachments through a native WebView resource handler that requests authenticated file chunks. Video uses ranges. Clipboard images enter the same staged upload path as other files.
- An additive migration stores profile metadata, per-owner contact notes, read cursors, opt-in receipts and quote references in PostgreSQL. The read cursor starts at existing history during upgrade. Account and conversation polling supplies unread counts. The active view requests receipt status only for its displayed sent messages. SQLite stores per-device hide markers independently from the sync cursor.

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
