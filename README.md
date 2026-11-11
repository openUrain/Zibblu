# Zibblu

**Chat freely. Share privately.**

A modern messenger with a clean interface, serious file sharing, opt-in LAN-only private chats, and AI assistants you control.

> **Status:** M1 normal chat and files. The LAN private chat and AI modules are planned.

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

M1 uses **Tauri 2 + React + TypeScript**, a **Rust/Tokio** client, per-account **SQLite** history, and one **Rust/PostgreSQL** service for authentication, message delivery, and attachments. Private sessions target **mDNS/DNS-SD discovery + authenticated QUIC** with fail-closed behavior.

## Run M1 on Windows

Install Rust 1.90 or newer, Node.js 22.12 or newer, PostgreSQL, and the [Tauri Windows prerequisites](https://v2.tauri.app/start/prerequisites/#windows). Use a Visual Studio x64 developer terminal to load MSVC and the Windows SDK. Create an empty PostgreSQL database owned by a dedicated Zibblu database user. From the repository root, start the service with that user's connection string.

```powershell
$env:DATABASE_URL = 'postgres://zibblu:<password>@127.0.0.1:5432/zibblu'
cargo run --locked -p zibblu-online
```

In another terminal, run the desktop app.

```powershell
cd apps/desktop
npm ci
npm run tauri dev
```

Create an account at `http://127.0.0.1:7841` and add another account by its handle in **Contacts**. For remote users, terminate HTTPS at a reverse proxy in front of the loopback service and give both clients the same HTTPS origin. Configure rate limiting for login and registration and keep the database reachable only by trusted services.

Use the paperclip or folder button to share an attachment. Dropping one file or folder into an open chat brings up a confirmation. Folders arrive as ZIP archives. **Shared files** in a chat and the global **Files** workspace show attachments and transfers. Save a download to an unused filename. If a transfer pauses or fails, choose **Resume** or **Retry** to continue.

For two independent local profiles, launch the Windows executable with a different `ZIBBLU_DATA_DIR` in each process. By default, the app uses its Windows local app data directory.

The focused HTTP and SQLite acceptance test uses a disposable PostgreSQL database.

```powershell
$env:ZIBBLU_TEST_DATABASE_URL = 'postgres://zibblu:<password>@127.0.0.1:5432/zibblu_test'
cargo test --locked -p zibblu-online --test m1 -- --nocapture
```

## Documentation

- [Product and UX](docs/PRODUCT.md)
- [Architecture and boundaries](docs/ARCHITECTURE.md)
- [Security and privacy](docs/SECURITY.md)
- [Roadmap and acceptance](docs/ROADMAP.md)
- [Decision log](docs/DECISIONS.md)
- [Codex working rules](AGENTS.md)
- [Pull request template](.github/PULL_REQUEST_TEMPLATE.md) — concise human-readable implementation and review report
- [Repository Skills](docs/SKILLS.md) — vendored Codex skills and usage boundaries

## Development workflow

**One module → one scoped implementation → minimal acceptance checks → PR/review.** Build no abstractions, compatibility layers, migrations or test suites without a concrete need.

Use the [stop-that-shit](https://github.com/lennney/stop-that-shit) contract in Codex task prompts (for example `$stop-that-shit change -- ...`). The Skill is now checked into `.agents/skills/` for repository discovery; the optional Guard and its Hooks are **not** installed or activated by this repository. See [Skills](docs/SKILLS.md).

---

**Zibblu** · *Chat freely. Share privately.*

Normal chat uses server-assisted delivery and storage. See [Security](docs/SECURITY.md) for the M1 boundary.
