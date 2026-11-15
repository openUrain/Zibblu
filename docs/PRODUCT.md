# Product specification

**Product:** Zibblu — *Chat freely. Share privately.*  
**Stage:** M1 normal chat and files, M2 private LAN text chat, and M3 scoped AI assistants on Windows.

**Goal:** Familiar everyday messaging, powerful file utilities, a truly separate LAN-only private mode and explicitly scoped AI assistance.

## Interface

**Authoritative cross-platform UI/UX design:** [DESIGN_SYSTEM.md](DESIGN_SYSTEM.md) (Owner-approved, 2026-11-14). The following are **target design rules**, not a claim that all v0.1.1 screens already comply.

- Chat behavior, chat-list hierarchy, profile and settings organization follow **WeChat**. File operation capabilities and media viewer follow **QQ**. Visual polish is **iMessage-first** with subtle **Snapchat** playfulness; overall presentation is Apple-like restraint.
- Main desktop navigation has three panes (nav / conversation list / content). **Chats · Contacts · Files** are direct entries. The top-left logo opens your profile. The bottom-left menu contains Settings, About, Feedback availability and Sign out. Phone uses four bottom tabs **Chats · Contacts · Files · Me**. Tablet uses a two-pane adaptive layout.
- Chat: retain **current message bubble visuals unchanged**. Use WeChat-style chat list and contextual operations (desktop right-click; mobile long-press). Composer uses WeChat functionality and iMessage visual treatment. Show only necessary message states.
- Files: the **Files view only lists ordinary attachments that cannot be directly previewed inside chat and must be downloaded**. Pictures and videos stay in **per-conversation chat/media history and media search**, with in-chat previews when supported. Ordinary file messages use **WeChat-style cards** with **progress directly in chat**. File actions and batch management follow QQ; the media viewer supports QQ-style navigation and save operations where available.
- Profile: concise WeChat-like summary, extended info at a second level. Settings: desktop split-panel, phone hierarchical lists.
- Normal app/chat uses a lightly lavender-tinted **light theme**. M2 private chat **keeps its existing separate dark theme**, same core controls and explicit LAN-only indicators. AI stays inside normal chat as an opt-in `@name` flow with conspicuous per-request authorization.
- Shared semantic tokens, system fonts, linear icons with filled selected state, small radii, mostly flat surfaces (glass only on temporary overlays), balanced density and natural moderate motion. Never use excessive decoration, crowded controls or inconsistent cross-platform interactions.
- The first visible message has a local 24-hour time label. A new label appears when consecutive messages cross a local date boundary or are more than 10 minutes apart.


## 01 — Chat & Files (normal mode)

On Windows, the standalone client defaults to **Zibblu Online** on a fresh profile. Sign in or create an account directly. **Advanced** allows a custom server, and upgrades keep the previously saved server and local profile. The client uses local SQLite. PostgreSQL runs with the server.

Registration asks for the new password twice. On Windows, the main window starts centered in the work area of its initial display. The first close asks whether to exit or hide to the system tray. Settings lets you change this local preference. Minimize still uses the taskbar. The tray can show the same window or exit the app. Launching again with the same data directory restores its window. Separate directories can run independent clients.

**User flow:** sign in → find a username → send a contact request → wait for approval → send messages or start a group → share files and find them in Files.

Initial functional scope:
- Direct and basic group chat, local message history and offline delivery through the online service.
- Usernames keep their registered capitalization. Sign-in and contact lookup are case-insensitive.
- Contact requests can include a greeting that becomes a normal message upon approval. Profiles show a handle and avatar to signed-in accounts, with the cover and bio available to approved contacts. Private address-book remarks take precedence across the app.
- The chat list shows unread counts. With receipts enabled by both accounts, the latest outgoing message in an open direct chat shows one check after a recipient device saves it, then two checks after the recipient views it in the foreground. Changing either preference clears eligibility for messages sent before that change. Group chats and earlier messages have no checkmarks. A message can be copied, quoted or hidden on this device. Drafts are kept per conversation in memory for the current sign-in.
- The sender can recall an ordinary message or shared attachment within five minutes. Participants receive a recall notice in the timeline.
- Sending files/folders; clear transfer state, file metadata, pause/resume and integrity verification. Folders are shared as ZIP downloads.
- Paste a clipboard image into the composer and send it as an attachment. Open inline images and videos in the chat's media viewer to browse and save them. If a format cannot be previewed or decoded, download it from the chat or viewer.
- The conversation drawer opens the peer's profile in a direct chat or shows members and the invite QR code in a group. It also opens text search, media history and the separate list of shared files for that conversation. The sidebar Files view searches ordinary attachments across chats, with newest first as the default and Transfers available for maintenance.
- Standard online routing is **separate** from private mode; ordinary messages can involve the server.

Avoid in v1: voice/video, social feeds, bots, automatic cloud-drive sync, complex admin dashboards, mobile before desktop stabilization.

### Desktop image formats

| Format | Avatar and cover | Chat attachment |
| --- | --- | --- |
| PNG, JPEG/JPG, BMP | Confirm to save a static PNG | Preview and save the original |
| GIF, WebP, animated PNG (APNG) | Confirm to save the first frame as a static PNG | Preview with animation where present and save the original |
| AVIF, HEIC/HEIF and other unsupported images | Choose a supported format | Download the original without an inline preview |

Image inputs for profiles, clipboard pastes and inline previews are capped at 16 MiB, 8192 pixels per side and 16 million pixels total. Animation preview also has frame and decoded pixel limits. Profile images are resized proportionally to fit 1024 × 1024 pixels and 2 MiB. Images that cannot be previewed stay in chat media history when their file type is recognized. Chat attachments can still be sent and saved under the ordinary file size limit.

## 02 — Private Chat (opt-in LAN-only mode)

**User flow:** open a direct conversation or select members from a shared group → send a private invitation → each receiving account accepts on any device → exchange text in a separate temporary view. An open private QR session provides a 15-minute join window for signed-in devices on the LAN.

Rules:
- Devices need direct UDP on the same subnet or between explicitly approved subnets inside a managed site. The app verifies account-bound device certificates before showing an invitation.
- **Nearby devices** in the top bar lists discoverable Zibblu instances after sign-in. A selected device can receive a private invitation without a shared conversation or contact relationship. The selected device must authenticate and accept before chatting. In an active session, open a participant's profile to send a separate contact request.
- A direct invitation shows the sender and **Accept** and **Decline**. A group invitation also shows the invitees. Accepted accounts can join as they respond, and a late account or device sees new text only.
- Invitations cover the app window. Windows shows a compact lower-right prompt while the main window is minimized to the taskbar. Accepting brings the private chat forward. Declining keeps the main window minimized. While hidden to the tray, pending invitations stay in the app.
- No server relay, online forwarding or unnoticed switch back to normal mode. Connection failure means private mode is unavailable.
- No private messages or attachments written to normal chat history. MVP private mode supports **text**; private file transfer requires a separate later decision.
- Switching views keeps the private session available alongside ordinary chats. Ending the session or quitting the host closes the channel and clears the in-memory conversation.
- No cloud AI, push-notification excerpts, search indexing, previews, analytics payloads or logging of private content.
- The app cannot retract screenshots, recipient copies or guarantee removal of OS memory/swap remnants. UX copy must not claim absolute erasure.

## 03 — AI Assistants (explicit read-only)

**User flow:** configure OpenAI, DeepSeek or Grok with a model, API key and custom assistant name → type `@name` in a normal chat → choose prompt only, selected text messages or a bounded recent range → optionally select individual text files → review the exact disclosure and send → receive answer.

Rules:
- No background access. Each request has a **fresh, one-time** authorization; it never inherits prior scopes.
- Read only the chosen message snapshot and any individually selected files. Never give the provider arbitrary filesystem paths.
- Assistant responses cannot overwrite message history or modify/delete local files. No execution, plugin actions, continuous agents or write tools.
- Remote-provider use must disclose that selected content is sent outside Zibblu. Keep credentials in secure local storage rather than source control.
- Private mode excludes cloud AI. A future on-device AI option needs a separate privacy review.
- Context is capped at 60 text messages and 256 KiB per request. The scope selector displays the exact outbound snapshot and destination before sending.
- Assistant mention names match case-insensitively and must be unique across configured providers.
- Replies appear chronologically in the conversation with a provider logo. The invoking user can use **Visible to** to share a reviewed reply with selected conversation members. Unshared replies stay in the current sign-in.
- Settings supports testing the API key and listing model IDs through the provider's HTTPS API. Enter a model ID manually when model discovery is unavailable. An advanced base URL can point to a compatible HTTPS Responses API.

## Brand

Name: **Zibblu**. Tagline: **Chat freely. Share privately.**  
Personality: **Apple-like restraint, warm and precise, lightly playful without visual clutter.**

- **Main hue:** Lavender. **Exact HEX is not yet Owner-approved**; candidate prototypes are listed in [DESIGN_SYSTEM.md](DESIGN_SYSTEM.md) and must be reviewed for accessible contrast before being frozen.
- **Normal mode:** light surfaces with very subtle lavender tint, flat presentation and restrained accents.
- **Private LAN mode:** preserve independent dark theme; use explicit text/icon security states instead of relying on color as a privacy claim.
- **Reference hierarchy:** WeChat for chat/profiles/settings; QQ for files and media controls; iMessage for visual effects, Snapchat for small touches of personality.
- **Exceptions:** existing message bubbles remain unchanged; the file/media separation and in-chat progress are non-negotiable without a new Owner decision.


## Success definition

The product must make it obvious whether a message is **normal online**, **private LAN-only**, or part of an **AI disclosure**. No background change of transport or data access may surprise the user.
