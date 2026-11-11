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

## Open decisions

1. Trustworthy authentication and account identity binding for LAN peers.
2. Online-message encryption protocol and key lifecycle; do not claim E2EE until implemented.
3. How group AI output is displayed and how group participants are informed about cloud disclosure.
4. Private file transfer, mobile clients and multi-device sync are **deferred**, not silently included.

## Change format

```text
YYYY-MM-DD · D-00X · Decision
Context:
Decision:
Consequences:
```
