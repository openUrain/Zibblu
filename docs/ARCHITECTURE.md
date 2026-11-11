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
- Private LAN: mDNS/DNS-SD for discovery; authenticated QUIC peer-to-peer channel (e.g., Quinn), with peer/contact binding. Discovery alone is not authentication.
- AI: provider configuration and a narrowly scoped `read_selected_context` flow. Providers receive the prepared snapshot, never a broad database handle or writable file system.

**Folder layout when code exists** (do not create placeholders):
```text
apps/desktop/       # Tauri UI and integration
crates/core/        # Rust transport, storage and capability boundaries
services/online/    # online service, introduced during Module 01 if needed
docs/               # living product and engineering decisions
```

Use fewer packages if implementation proves simpler.

## Data ownership and flow

| Data | Normal chat | Private chat | AI request |
| --- | --- | --- | --- |
| Message text | persisted in normal store; online delivery as needed | memory-only for active session; never copied into normal store | bounded in-memory snapshot after user's explicit choice |
| Files | local disk + normal transfer pipeline | not in initial private MVP | only specifically selected files, read-only |
| Keys | stored using suitable identity/key storage | per-session secrets cleared on end | provider credentials stored securely; never in prompts/logs |
| Cloud | normal chat service allowed | **not allowed for session payload/relay** | only selected content sent to explicitly chosen remote provider |

Important: ordinary online messaging should target authenticated, encrypted transport and later end-to-end encrypted message content. Do not advertise full E2EE until a concrete protocol, key lifecycle and verification exist.

## Transport state machine

```text
Normal chat ──[tap private]──> Discovering
Discovering ──[peer found + both consent + auth]──> Private connected
Discovering ──[failure/timeout/refusal]──> Normal chat (no private data sent)
Private connected ──[end/disconnect/timeout]──> Clear ephemeral state ──> Normal chat
```

Private data does **not** travel on the normal channel during discovery, failure, reconnection or cleanup. Explicit confirmation is required for a new private session.

## Boundaries worth keeping strict

1. The private-session transport API has **no relay endpoint** and no permission to write a transcript to the persistent chat store.
2. AI context extraction is a separate user-confirmed operation; it has **read-only capabilities**, no arbitrary filesystem handle and no chat database write handle.
3. File streaming should be chunked/resumable with transfer identity and integrity verification; do not overbuild distributed storage or deduplication.
4. Only introduce offline queues, migrations, reconnection complexity and cross-device syncing when the current module's user journeys require them.

## Design trade-offs pending implementation

- Exact account/identity and group synchronization design.
- Choice and audit of an established E2EE protocol for online messages.
- Private LAN authentication UX (trusted key, fingerprint or peer verification).
- Mobile background constraints and platform-specific discovery.
- Provider-specific AI usage disclosure and optional on-device models.

Record decisions in [DECISIONS.md](DECISIONS.md) when one is actually made.
