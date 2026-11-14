# Zibblu

**Chat freely. Share privately.**

[![Zibblu CI](https://github.com/openUrain/Zibblu/actions/workflows/ci.yml/badge.svg)](https://github.com/openUrain/Zibblu/actions/workflows/ci.yml)

A modern messenger with a clean interface, serious file sharing, opt-in LAN-only private chats, and AI assistants you control.

> **Status:** M1 normal chat and files, M2 LAN private text chat, and M3 scoped AI assistants on Windows.

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

## Install Zibblu on Windows

1. On Windows 10 or 11 x64, download and run `Zibblu_0.1.2_x64_Client_Setup.exe` from the [v0.1.2 release](https://github.com/openUrain/Zibblu/releases/tag/v0.1.2). Setup installs the desktop client for the current user. If Microsoft WebView2 is missing, setup downloads and installs it. No PostgreSQL, Rust, Docker, or local chat server is needed.
2. Open **Zibblu** from the Start menu. **Zibblu Online** selects the official server at `https://zibblu-api.openurain.com`. Create an account or sign in. The app shows **Connected** once it has synchronized with the server.
3. In **Contacts**, find a friend by username and send a contact request. After approval you can start a conversation and share files.

The official service currently runs on the owner's Windows host through Cloudflare Tunnel. When that host is offline, sign-in and online chat are unavailable. Ordinary chat uses server-assisted delivery and storage without end-to-end encryption. [Security boundaries](docs/SECURITY.md)

Use **Advanced** on the sign-in screen for a custom HTTPS origin without a path. Local development also supports `http://127.0.0.1:7841`. An upgrade keeps the last saved server, including the old loopback address. To switch to the official server, choose **Advanced → Use Zibblu Online** and sign in to an account there. The new choice is remembered after successful sign-in. Local histories stay separate by server and account. If you use an existing LAN Host or `Join-Zibblu.cmd` profile, keep using that launcher for its data directory and certificate configuration.

The [v0.1.0 LAN Host release](https://github.com/openUrain/Zibblu/releases/tag/v0.1.0) is the older self-hosted package. It bundles PostgreSQL and the chat service and creates `Zibblu-Join.zip` for LAN users.

## Development and self-hosting

[Deploy `zibblu-online` with Docker Compose or migrate an existing PostgreSQL deployment.](docs/ONLINE_DEPLOYMENT.md)

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

Open **Advanced**, enter `http://127.0.0.1:7841`, and create a test account. For remote users, terminate HTTPS at a reverse proxy in front of the loopback service and give both clients the same HTTPS origin. Configure rate limiting for login and registration before public access and keep the database reachable only by trusted services.

Use the paperclip, folder button, drag and drop, or paste an image into the composer. Folders arrive as ZIP archives. Use the conversation's **…** menu for **Search chat** and **Shared files**. The sidebar **Files** workspace shows all shared files by time, with filename search and transfer controls. Save a download to an unused filename. If a transfer pauses or fails, choose **Resume** or **Retry** to continue.

For two independent local profiles, launch the Windows executable with a different `ZIBBLU_DATA_DIR` in each process. By default, the app uses its Windows local app data directory.

### Build and check the Windows client installer

From an x64 developer terminal in `apps/desktop`, run:

```powershell
npm ci
npm run tauri build -- --bundles nsis --target x86_64-pc-windows-msvc -- --locked
```

The setup executable is `Zibblu_0.1.2_x64-setup.exe` under `target/x86_64-pc-windows-msvc/release/bundle/nsis` at the repository root. The release asset is named `Zibblu_0.1.2_x64_Client_Setup.exe`. The installer preserves the existing Windows installer identity and defaults to the data directory `%LOCALAPPDATA%\com.zibblu.desktop`, separate from the installed executable.

On a clean Windows 10 or 11 system, install and sign in with a test account. Change the theme, open a normal chat and save an attachment. Minimize and restore the app from the taskbar, then close it with **X** and start it again. Sign in and check that the server, theme and chat history are still available. With the app closed, run setup again and repeat that check. Uninstall without selecting the option to delete app data, reinstall, and check that the profile is still present. Use a small display or higher DPI to check that the native file and invitation controls remain reachable.

The focused HTTP and SQLite acceptance test uses a disposable PostgreSQL database.

```powershell
$env:ZIBBLU_TEST_DATABASE_URL = 'postgres://zibblu:<password>@127.0.0.1:5432/zibblu_test'
cargo test --locked -p zibblu-online --test m1 -- --nocapture
```

## Try a private LAN chat

Use the same Zibblu HTTPS server on two Windows devices signed in to different test accounts. On a trusted home or office subnet, allow Zibblu through Windows Defender Firewall for the Private profile. Discovery needs mDNS (UDP 5353) or configured site DNS-SD, and private chat needs direct UDP. Open **Nearby devices** on both devices before adding contacts. Each device should appear with its identity unverified. Create a shared conversation and select the matching account to invite. Check that both devices appear again after switching between Wi-Fi and Ethernet. Move one device outside the allowed LAN and check that a new private connection fails.

The **ghost** in a normal conversation also starts a private invitation and lets you choose invitees in a group. The invitation covers the Zibblu window. When that window is minimized on Windows, a compact invitation appears above the taskbar at the lower right. Accepting opens the private chat. Declining leaves the main window minimized. A valid response applies to that account's devices. Participants can switch to another conversation and return without ending the session. Minimizing keeps Zibblu running on the taskbar. Closing the main window exits Zibblu and ends local private sessions.

Use **Start private QR chat** for an open, temporary session. Anyone signed in to the same server with the code and a permitted LAN connection can join during the 15-minute window. Late accounts and devices see live text from their own join point. Use **Scan a QR code** with a camera or saved image. Your contact code is in **Settings**. A normal group's header provides its join code. Creating a new normal group code replaces the prior one. Joining a normal group grants access to messages and attachments published after admission.

Zibblu postpones invitations when Windows reports that notifications are unavailable, such as during screen lock or Windows presentation mode. Use **Pause private invitation pop-ups** in Settings for Do Not Disturb or screen sharing that Windows does not report to desktop apps. A named invitation without an accepted participant expires after 90 seconds.

For the focused check, receive an invite with Zibblu open, then repeat with the main window minimized. Check the in-app overlay and the lower-right prompt, including decline while minimized. Use two recipient devices on the same account and a third account. Accept on one device, let the other join, and check that a private message sent before the second device joined is absent there. Invite two members from a group, accept at different times and check that a non-invited member receives nothing. Check a QR scan before and after expiration. Cancel while an invitation is paused and check for a missed item after resuming. End the session or exit the hosting app, then check that reopening restores only normal history.

### Managed company networks

For two internal subnets that IT routes directly within one site, set `ZIBBLU_LAN_SITE_SUBNETS` on each desktop to the approved comma-separated private IPv4 CIDRs, for example `10.20.10.0/24,10.20.20.0/24`. Keep this setting within the local site. Set `ZIBBLU_LAN_PORT` to a fixed UDP port such as `7842` to scope the bidirectional firewall rules for the approved subnets. Clients without that setting use an ephemeral port. A private QR code carries a direct endpoint. For automatic cross-subnet invitations, configure your internal DNS-SD discovery proxy and set `ZIBBLU_LAN_DISCOVERY_DOMAIN` to its service domain. The client browses `_zibblu-private._udp.<domain>.`. Both endpoints must be directly routable. Separate routers with NAT or client isolation need routing and firewall changes from IT.

Verify across two approved VLANs, then repeat from a subnet or firewall segment outside the configured policy and confirm neither the invitation nor the QR establishes a private connection there. The server supplies device certificates through the existing authenticated sign-in service. Deploy the updated server before enabling private chats on upgraded clients.

## Ask a read-only AI assistant

In **Settings → AI assistants**, choose OpenAI, DeepSeek, or Grok. Set a mention name, model ID and your own provider API key. **Test connection** lists models when the provider supports it. The model ID remains editable. **Advanced connection settings** accepts an HTTPS API base URL. Windows protects the key in the local Zibblu account profile.

Start a normal chat message with `@name` and enter a prompt. The context screen lets you use the prompt alone, pick up to 60 text messages from the current chat, or include a recent range. Add up to three individual UTF-8 text files of 32 KiB each. The combined snapshot is capped at 256 KiB. Review the exact text and destination on that screen and send over HTTPS.

Replies stay in memory for your sign-in. To share one, use **Visible to** on the reply and select the chat members who should receive it. The shared reply appears in those members' timelines. Provider logos identify [OpenAI](https://openai.com/brand/), [DeepSeek](https://github.com/deepseek-ai/awesome-deepseek-integration/tree/main/docs/_logo%20svg) and [Grok](https://x.ai/legal/brand-guidelines), and belong to their respective owners.

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

**One module → one scoped implementation → minimal acceptance checks → PR → CI + Codex review → Owner merge.** Build no abstractions, compatibility layers, migrations or test suites without a concrete need.

The [GitHub Actions CI workflow](.github/workflows/ci.yml) runs on PRs targeting `main` and pushes to `main`. It currently checks **M1 Rust/PostgreSQL integration**, **React/TypeScript build**, and **Windows Tauri Rust compilation** (after building the frontend assets). These checks do **not** verify an installer, real Windows GUI behavior, or M2 LAN-only peer communication. Record those acceptance results separately in each PR.

Codex automatic review and CI are complementary: review comments are not passing test results. Before merging, the Owner checks relevant CI results, outstanding actionable review findings and the PR's known limitations. Branch protection for this private GitHub Free organization repository is currently **not enforced**; these checks are an Owner-maintained process, not a GitHub-enforced merge gate.

Use the [stop-that-shit](https://github.com/lennney/stop-that-shit) contract in Codex task prompts (for example `$stop-that-shit change -- ...`). The Skill is now checked into `.agents/skills/` for repository discovery; the optional Guard and its Hooks are **not** installed or activated by this repository. See [Skills](docs/SKILLS.md).

---

**Zibblu** · *Chat freely. Share privately.*

Normal chat uses server-assisted delivery and storage. See [Security](docs/SECURITY.md) for the M1 boundary.
