# Zibblu

**Chat freely. Share privately.**

A modern messenger with a clean interface, serious file sharing, opt-in LAN-only private chats, and AI assistants you control.

> **Status:** Design / planning. No app functionality is implemented yet.

## What makes Zibblu different?

| Module | What it does | Core promise |
| --- | --- | --- |
| **01 / Chat & Files** | Everyday chats, groups, message history and reliable large-file sharing with a dedicated files view. | Familiar, simple UX with practical file tools. |
| **02 / Private Chat** | Tap the lock button to start a temporary peer-to-peer session with a contact on the same reachable LAN. | No message payloads on Zibblu servers, no internet fallback, no persistent private history. |
| **03 / AI Assistants** | Call a custom-named AI using `@name`; explicitly select the messages and optional files it may read. | One request, one bounded read-only authorization. No edits, background access or autonomous actions. |

### A few non-negotiables

- **Simple by default.** Keep the main chat surface quiet. Put advanced file controls in the file workspace.
- **Private is a separate mode.** Both participants must agree. A failed LAN connection never falls back to the internet.
- **Clear records on exit.** Private-session content is kept out of persistent app storage and cleared when the session ends; OS-level remnants, screenshots and recipient captures cannot be guaranteed removable.
- **AI is opt-in.** It sees only user-selected context for that invocation. Cloud AI is unavailable for private chats.
- **Build what is needed.** Finish one complete module, then run the smallest useful checks, including its critical privacy/permission failure cases.

## Intended technology

Desktop-first: **Tauri 2 + React + TypeScript**, with a **Rust/Tokio** messaging and transfer core. Normal-chat data uses **SQLite**; an online **Rust/PostgreSQL** service will be added only when Module 01 needs remote delivery. Private sessions target **mDNS/DNS-SD discovery + authenticated QUIC** with fail-closed behavior.

These are working choices, not a reason to scaffold unused services or introduce dependencies prematurely.

## Documentation

- [Product and UX](docs/PRODUCT.md)
- [Architecture and boundaries](docs/ARCHITECTURE.md)
- [Security and privacy](docs/SECURITY.md)
- [Roadmap and acceptance](docs/ROADMAP.md)
- [Decision log](docs/DECISIONS.md)
- [Codex working rules](AGENTS.md)

## Development workflow

**One module → one scoped implementation → minimal acceptance checks → PR/review.** Build no abstractions, compatibility layers, migrations or test suites without a concrete need.

Use the [stop-that-shit](https://github.com/lennney/stop-that-shit) contract in Codex task prompts (for example `$stop-that-shit change -- ...`). The skill and optional Guard must be installed in Codex: merely mentioning them in this repository does not activate runtime enforcement.

---

**Zibblu** · *Chat freely. Share privately.*

Features and security properties in this README are design goals until independently verified in a working build.
