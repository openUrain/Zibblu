# Product specification

**Product:** Zibblu — *Chat freely. Share privately.*  
**Stage:** M1 normal chat and files, M2 private LAN text chat, and M3 scoped AI assistants on Windows.

**Goal:** Familiar everyday messaging, powerful file utilities, a truly separate LAN-only private mode and explicitly scoped AI assistance.

## Interface

Main navigation: **Chats · Contacts · Files · Settings**.

- Chat view: uncluttered conversation list and composer; attachment action and a small **ghost** entry for private sessions.
- Files view: search, type and time order for a conversation; the global view also filters by sharer. Both default to newest first and show the sharer's avatar and name.
- AI is invoked **inside the existing composer** with `@custom-name`; it is not a constantly running participant or an autonomous agent.
- The app and ordinary chats use a light palette. Private conversations use a dark palette and the same message and composer controls. Message groups are labeled in local 24-hour time, with a new group on day changes or after five minutes.

## 01 — Chat & Files (normal mode)

On Windows, the standalone client defaults to **Zibblu Online** on a fresh profile. Sign in or create an account directly. **Advanced** allows a custom server, and upgrades keep the previously saved server and local profile. The client uses local SQLite. PostgreSQL runs with the server.

**User flow:** sign in → find a username → send a contact request → wait for approval → send messages or start a group → share files and find them in Files.

Initial functional scope:
- Direct and basic group chat, local message history and offline delivery through the online service.
- Usernames keep their registered capitalization. Sign-in and contact lookup are case-insensitive.
- Contact requests can include a greeting that becomes a normal message upon approval. Profiles show a handle and avatar to signed-in accounts, with the cover and bio available to approved contacts. Open your own profile from the rail avatar. Private address-book remarks take precedence across the app.
- The chat list shows unread counts. Optional account-level read receipts require both people to opt in. A message can be copied, quoted or hidden on this device. Drafts are kept per conversation in memory for the current sign-in.
- The sender can recall an ordinary message or shared attachment within five minutes. Participants receive a recall notice in the timeline.
- Sending files/folders; clear transfer state, file metadata, pause/resume and integrity verification. Folders are shared as ZIP downloads.
- Paste a clipboard image into the composer and send it as an attachment. Supported images and video frames appear inline when the Windows WebView can decode them.
- The conversation menu provides text search and files already shared in that chat. The sidebar Files view covers all chats, newest first, with filename search and transfer controls.
- Standard online routing is **separate** from private mode; ordinary messages can involve the server.

Avoid in v1: voice/video, social feeds, bots, automatic cloud-drive sync, complex admin dashboards, mobile before desktop stabilization.

## 02 — Private Chat (opt-in LAN-only mode)

**User flow:** open a direct conversation or select members from a shared group → send a private invitation → each receiving account accepts on any device → exchange text in a separate temporary view. An open private QR session provides a 15-minute join window for signed-in devices on the LAN.

Rules:
- Devices need direct UDP on the same subnet or between explicitly approved subnets inside a managed site. The app verifies account-bound device certificates before showing an invitation.
- **Nearby devices** in the top bar lists discoverable Zibblu instances after sign-in. A selected device can receive a private invitation without a shared conversation or contact relationship. The selected device must authenticate and accept before chatting. In an active session, open a participant's profile to send a separate contact request.
- A direct invitation shows the sender and **Accept** and **Decline**. A group invitation also shows the invitees. Accepted accounts can join as they respond, and a late account or device sees new text only.
- Invitations cover the app window. Windows shows a compact lower-right prompt while the main window is minimized. Accepting brings the private chat forward. Declining keeps the main window minimized.
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
Personality: warm, quick, playful without visual clutter.  
Working palette (not implementation requirements): primary `#5865F2`, local-private accent `#35D4FF`, light `#F7F8FC`, dark `#12141D`. Avoid a visual color cue as the sole privacy-state indicator.

## Success definition

The product must make it obvious whether a message is **normal online**, **private LAN-only**, or part of an **AI disclosure**. No background change of transport or data access may surprise the user.
