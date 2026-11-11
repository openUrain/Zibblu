# Product specification

**Product:** Zibblu — *Chat freely. Share privately.*  
**Stage:** Pre-implementation; desktop-first (Windows initially).  
**Goal:** Familiar everyday messaging, powerful file utilities, a truly separate LAN-only private mode and explicitly scoped AI assistance.

## Interface

Main navigation: **Chats · Contacts · Files · Settings**.

- Chat view: uncluttered conversation list and composer; attachment action and a small **lock** entry for private sessions.
- Files view: per-conversation files, global filters by person/type/date, preview where feasible, download location and transfer progress.
- AI is invoked **inside the existing composer** with `@custom-name`; it is not a constantly running participant or an autonomous agent.
- Light/dark themes; playful Zibblu brand but calm, readable message surfaces. Use ordinary system accessibility controls.

## 01 — Chat & Files (normal mode)

**User flow:** sign in → find/add a contact → send text → optionally start a group → drag/drop a file → inspect progress → resume an interrupted transfer → find it in Files.

Initial functional scope:
- Direct and basic group chat, local message history and offline delivery through the online service.
- Sending files/folders; clear transfer state, file metadata, pause/resume and integrity verification.
- Dedicated Files page and per-chat file history. Failure/retry is visible, not silently hidden.
- Standard online routing is **separate** from private mode; ordinary messages can involve the server.

Avoid in v1: voice/video, social feeds, bots, automatic cloud-drive sync, complex admin dashboards, mobile before desktop stabilization.

## 02 — Private Chat (opt-in LAN-only mode)

**User flow:** open a normal conversation → tap lock → discover reachable peer on LAN → both explicitly accept → enter separate temporary conversation → either party ends → session content clears.

Rules:
- Both users must be on a mutually reachable local network. Same Wi-Fi name is not sufficient proof.
- Authenticate the intended contact and secure peer transport before showing **Private connected**.
- No server relay, online forwarding or unnoticed switch back to normal mode. Connection failure means private mode is unavailable.
- No private messages or attachments written to normal chat history. MVP private mode supports **text**; private file transfer requires a separate later decision.
- Ending closes the channel, clears in-memory messages/keys and attempts to notify the peer. Timeouts/crash/restart must not restore a private transcript.
- No cloud AI, push-notification excerpts, search indexing, previews, analytics payloads or logging of private content.
- The app cannot retract screenshots, recipient copies or guarantee removal of OS memory/swap remnants. UX copy must not claim absolute erasure.

## 03 — AI Assistants (explicit read-only)

**User flow:** configure provider and custom assistant name → type `@name` → choose context (current prompt / selected messages / bounded recent or time range) → optionally select specific files → review disclosure and send → receive answer.

Rules:
- No background access. Each request has a **fresh, one-time** authorization; it never inherits prior scopes.
- Read only the chosen message snapshot and any individually selected files. Never give the provider arbitrary filesystem paths.
- Assistant responses cannot overwrite message history or modify/delete local files. No execution, plugin actions, continuous agents or write tools.
- Remote-provider use must disclose that selected content is sent outside Zibblu. Keep credentials in secure local storage rather than source control.
- Private mode excludes cloud AI. A future on-device AI option needs a separate privacy review.
- In groups, make it clear when a response will be visible to everyone; only the invoking user grants context from messages they can access.

## Brand

Name: **Zibblu**. Tagline: **Chat freely. Share privately.**  
Personality: warm, quick, playful without visual clutter.  
Working palette (not implementation requirements): primary `#5865F2`, local-private accent `#35D4FF`, light `#F7F8FC`, dark `#12141D`. Avoid a visual color cue as the sole privacy-state indicator.

## Success definition

The product must make it obvious whether a message is **normal online**, **private LAN-only**, or part of an **AI disclosure**. No background change of transport or data access may surprise the user.
