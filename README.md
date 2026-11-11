# Zibblu

**Chat freely. Share privately.**

A modern messenger with a clean interface, serious file sharing, opt-in LAN-only private chats, and AI assistants you control.

> **Status:** M1 normal chat and files and M2 LAN private text chat. The AI module is planned.

## What makes Zibblu different?

| Module | What it does | Core promise |
| --- | --- | --- |
| **01 / Chat & Files** | Everyday chats, groups, message history and reliable large-file sharing with a dedicated files view. | Familiar, simple UX with practical file tools. |
| **02 / Private Chat** | Tap the ghost icon to invite people from a chat or start a temporary QR session on a reachable LAN. | No message payloads on Zibblu servers, no internet fallback, no persistent private history. |
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

## Try a private LAN chat

Use the same Zibblu server on two Windows devices. On a regular home or office subnet, allow mDNS (UDP 5353) and Zibblu's direct UDP traffic on the private network. Start from a shared direct conversation or group. The person starting the session selects the **ghost** and, in a group, chooses the invitees. The invitation covers the Zibblu window. When that window is minimized on Windows, a compact invitation appears above the taskbar at the lower right. Accepting opens the private chat. Declining leaves the main window minimized. A valid response applies to that account's devices. Participants can switch to another conversation and return without ending the session.

Use **Start private QR chat** for an open, temporary session. Anyone signed in to the same server with the code and a permitted LAN connection can join during the 15-minute window. Late accounts and devices see live text from their own join point. Use **Scan a QR code** with a camera or saved image. Your contact code is in **Settings**. A normal group's header provides its join code. Creating a new normal group code replaces the prior one. Joining a normal group grants access to messages and attachments published after admission.

Zibblu postpones invitations when Windows reports that notifications are unavailable, such as during screen lock or Windows presentation mode. Use **Pause private invitation pop-ups** in Settings for Do Not Disturb or screen sharing that Windows does not report to desktop apps. A named invitation without an accepted participant expires after 90 seconds.

For the focused check, receive an invite with Zibblu open, then repeat with the main window minimized. Check the in-app overlay and the lower-right prompt, including decline while minimized. Use two recipient devices on the same account and a third account. Accept on one device, let the other join, and check that a private message sent before the second device joined is absent there. Invite two members from a group, accept at different times and check that a non-invited member receives nothing. Check a QR scan before and after expiration. Cancel while an invitation is paused and check for a missed item after resuming. End the session or exit the hosting app, then check that reopening restores only normal history.

### Managed company networks

For two internal subnets that IT routes directly within one site, set `ZIBBLU_LAN_SITE_SUBNETS` on each desktop to the approved comma-separated private IPv4 CIDRs, for example `10.20.10.0/24,10.20.20.0/24`. Keep this setting within the local site. Set `ZIBBLU_LAN_PORT` to a fixed UDP port such as `7842` to scope the bidirectional firewall rules for the approved subnets. Clients without that setting use an ephemeral port. A private QR code carries a direct endpoint. For automatic cross-subnet invitations, configure your internal DNS-SD discovery proxy and set `ZIBBLU_LAN_DISCOVERY_DOMAIN` to its service domain. The client browses `_zibblu-private._udp.<domain>.`. Both endpoints must be directly routable. Separate routers with NAT or client isolation need routing and firewall changes from IT.

Verify across two approved VLANs, then repeat from a subnet or firewall segment outside the configured policy and confirm neither the invitation nor the QR establishes a private connection there. The server supplies device certificates through the existing authenticated sign-in service. Deploy the updated server before enabling private chats on upgraded clients.

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
