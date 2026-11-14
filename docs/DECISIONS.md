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


## Open decisions

1. Online-message encryption protocol and key lifecycle; do not claim E2EE until implemented.
2. Private file transfer, mobile clients and normal-chat multi-device sync are deferred.

## Change format

```text
YYYY-MM-DD · D-00X · Decision
Context:
Decision:
Consequences:
```
