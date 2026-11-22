# Product specification

**Product:** Zibblu — *Chat freely. Share privately.*  
**Release baseline (2026-11-20):** Windows has reached public GitHub Release **v0.1.6**. The repository also publishes an **unsigned iPhone test preview**; it is not App Store distribution, and physical-device acceptance must be reported separately. This document describes product behavior, not a guarantee that every platform feature has passed live-device testing. Confirm current downloadable assets from [releases](https://github.com/openUrain/Zibblu/releases) and [README](../README.md).

**Goal:** Give organizations a choice of server for everyday chats and file sharing. Give participants a separate, consent-based mode for private text on a reachable LAN.

## Enterprise Self-Hosting

Organizations can [deploy the v2 online service and PostgreSQL on their own infrastructure](ONLINE_DEPLOYMENT.md#enterprise-self-hosting). Windows clients select that service’s HTTPS origin under **Advanced** at sign-in. Ordinary messages and attachments between accounts on the same self-hosted service go through that service, without routing through the official Zibblu server.

The selected server handles accounts, contacts, groups, devices and delivery. It can read ordinary chat payloads during online availability because ordinary chats have no end-to-end encryption. Long-term received history is stored in the client’s per-server, per-account SQLite profile. Private LAN text uses its own direct connection. An explicitly authorized remote AI request sends the reviewed context to the selected provider. [Security boundaries](SECURITY.md)

## Interface

The [current cross-platform design system](DESIGN_SYSTEM.md) preserves Zibblu-specific behavior and branding; **Always UI** supplies the latest UI design, numeric-preset and review workflow. [Issue #54](https://github.com/openUrain/Zibblu/issues/54) is historical evidence; [Issue #59](https://github.com/openUrain/Zibblu/issues/59) tracks subsequent Windows refinements.

- Windows, iPhone and iPad each follow WeChat on that platform for navigation, chat, profile and settings. Zibblu uses its own imagery and Lavender accents.
- Desktop has three panes for navigation, conversation list and content. **Chats · Contacts · Files** are direct entries. The signed-in user's avatar opens their profile. The bottom-left menu contains Settings, About, Feedback availability and Sign out. iPhone uses **Chats · Contacts · Files · Me**. iPad uses list and detail panes when the window fits and pushes detail onto the navigation stack in narrow windows.
- Desktop shows a selected contact beside the searchable contact list. Use **Add contact** to look up an account on the server. Opening a chat from the list focuses the composer. Enter sends and Shift+Enter inserts a new line. On Windows, the composer starts at one line, grows up to six lines, and scrolls internally for longer drafts.
- Chat shows the live group roster count in the group heading. Actions open by right-click on desktop and long press on touch. Only the active input area and related scroll region adjust to the keyboard.
- Files lists ordinary attachments that require a download. Chat previews and conversation media history contain pictures and videos. Transfer progress, save and retry stay with the chat attachment. System pickers and export sheets handle file access.
- On Windows, the signed-in user's avatar opens a compact profile card beside the navigation rail. The card keeps the current conversation or contact visible behind it. Editors and the personal QR code return to the layer that opened them. iPhone and iPad use their profile navigation. Select and crop a photo before confirming. Display names are editable while the sign-in handle stays stable. Profile, image and contact data render from the signed-in account's cache first and update in the background.
- Normal app/chat uses a lightly lavender-tinted **light theme**. M2 private chat **keeps its existing separate dark theme**, same core controls and explicit LAN-only indicators. AI stays inside normal chat as an opt-in `@name` flow with conspicuous per-request authorization.
- Preserve the existing semantic tokens, platform-native fonts, Zibblu original icons, Lavender identity, small-radius and approved floating-label inputs. Use Always UI `creative/design/improve` and independent `check` as specified by [DESIGN_SYSTEM](DESIGN_SYSTEM.md); follow the required references, existing-token-first numeric presets and evidence-based visual discipline rather than copying an obsolete universal visual recipe.
- The first visible message has a local 24-hour time label. A new label appears when consecutive messages cross a local date boundary or are more than 10 minutes apart.


## 01 — Chat & Files (normal mode)

普通聊天以当前 Windows 设备的本地历史为准。已登录时断网可以查阅本机历史，普通文本和已暂存的附件会排队。重连后使用原发送键继续。新设备只接收有效的账号积压及之后的投递。离线文字默认可投递 72 小时，错过期限时显示过期占位。文件下载期限由部署配置决定，保存并验证的本机文件可继续打开和另存。[D-018](DECISIONS.md#d-018--m1-local-first-与临时文件投递)

On Windows, the standalone client defaults to **Zibblu Online** on a fresh profile. Sign in or create an account directly. **Advanced** allows a custom server, and the app remembers the selected server. The v0.1.4 client opens a separate profile from earlier versions and requires a v2 service. The client uses local SQLite. PostgreSQL runs with the server.

On Windows, a normal app exit leaves the last successful sign-in available. On the next start, select the saved account card to ask its server to restore the session, or choose **Use another account** for password sign-in. The card uses the last locally saved avatar or an initial. A network failure allows a retry. An expired or revoked session returns to password sign-in. **Sign out** removes the saved sign-in from this device. Settings lists the account’s active devices and can retire another installation.

iPhone and iPad use the same sign-in and sync flows. Choose a file from the system document picker and profile images from the system photo picker. Downloads stay in the app sandbox until you choose **Share** to export a verified file. To share a directory, zip it and choose the archive in Files.

Registration asks for the new password twice. On Windows, the main window starts centered in the work area of its initial display. The first close asks whether to exit or hide to the system tray. Settings lets you change this local preference. Minimize still uses the taskbar. The tray can show the same window or exit the app. Launching again with the same data directory restores its window. Separate directories can run independent clients.

**User flow:** sign in → find a username → send a contact request → wait for approval → send messages or start a group → share files and find them in Files.

Initial functional scope:
- Direct and basic group chat, local message history and offline delivery through the online service.
- Normal chat text supports up to 64 KiB of UTF-8 per message with a compatible online service. The composer keeps over-limit text available for editing and shows the limit.
- Usernames keep their registered capitalization. Sign-in and contact lookup are case-insensitive.
- Contact requests can include a greeting that becomes a normal message upon approval. Profiles show a handle and avatar to signed-in accounts, with the cover and bio available to approved contacts. Private address-book remarks take precedence across the app.
- The chat list shows unread counts. With receipts enabled by both accounts, the latest outgoing message in an open direct chat shows one check after a recipient device saves it, then two checks after the recipient views it in the foreground. Changing either preference clears eligibility for messages sent before that change. Group chats and earlier messages have no checkmarks. A message can be copied, quoted or hidden on this device. Drafts are kept per conversation in memory for the current sign-in.
- The sender can recall an ordinary message or shared attachment within five minutes. Participants receive a recall notice in the timeline.
- Sending files/folders with persistent transfer progress, pause/resume and SHA-256 verification. Folders are shared as ZIP downloads. An expired online copy shows a resend prompt when the device has no verified copy.
- On Windows, the composer’s **+** menu opens the file and folder pickers. Copy up to eight files or folders in File Explorer and paste into the composer, then confirm the filenames and destination chat before sharing. Use Ctrl+V or the composer's **Paste** action to add a clipboard image. Images open in a separate window. Scroll at a point to zoom, drag an enlarged image to pan, and double-click to switch between fit and original size. Videos open in the chat media viewer. If a format cannot be previewed or decoded, save the original attachment.
- On Windows, the conversation drawer opens below the chat header and overlays only the right side of the conversation. It opens the peer's profile in a direct chat or shows the synced members and invite QR code in a group. It also opens text search, media history and shared files for that conversation. Returning to the drawer retains the selected conversation and draft. The sidebar Files view searches ordinary attachments across chats, with newest first as the default. File transfer progress and recovery actions appear beside the corresponding file. Open a saved file using its Windows default app. Manage unfinished media downloads in the conversation's media history.
- Standard online routing is **separate** from private mode; ordinary messages can involve the server.

Avoid in v1: voice/video, social feeds, bots, automatic cloud-drive sync, complex admin dashboards, mobile before desktop stabilization.

### Desktop image formats

| Format | Avatar and cover | Chat attachment |
| --- | --- | --- |
| PNG, JPEG/JPG, BMP | Confirm to save a static PNG | Preview and save the original |
| GIF, WebP, animated PNG (APNG) | Confirm to save the first frame as a static PNG | Preview with animation where present and save the original |
| AVIF, HEIC/HEIF and other unsupported images | Choose a supported format | Download the original without an inline preview |

Image inputs for profiles, clipboard pastes and inline previews are capped at 16 MiB and 8192 pixels per side. Static PNG previews allow up to 16,777,216 pixels, including 4096 × 4096 images. Other inputs allow up to 16 million pixels. Animation preview also has frame and decoded pixel limits. Profile images are resized proportionally to fit 1024 × 1024 pixels and 2 MiB. Images that cannot be previewed stay in chat media history when their file type is recognized. Chat attachments can still be sent and saved under the ordinary file size limit.

## 02 — Private Chat (opt-in LAN-only mode)

**User flow:** open a direct conversation or select members from a shared group → send a private invitation → each receiving account accepts on any device → exchange text in a separate temporary view. An open private QR session provides a 15-minute join window for signed-in devices on the LAN.

Rules:
- Devices need direct UDP on the same subnet or between explicitly approved subnets inside a managed site. The app verifies account-bound device certificates before showing an invitation.
- **Nearby** lists discoverable Zibblu instances after sign-in. On Windows, open it from the chat list’s search-side plus menu or the top bar. A selected device can receive a private invitation without a shared conversation or contact relationship. The selected device must authenticate and accept before chatting. In an active session, open a participant's profile to send a separate contact request.
- A direct invitation shows the sender and **Accept** and **Decline**. A group invitation also shows the invitees. Accepted accounts can join as they respond, and a late account or device sees new text only.
- Invitations cover the app window. Windows shows a compact lower-right prompt while the main window is minimized to the taskbar. Accepting brings the private chat forward. Declining keeps the main window minimized. While hidden to the tray, pending invitations stay in the app.
- No server relay, online forwarding or unnoticed switch back to normal mode. Connection failure means private mode is unavailable.
- No private messages or attachments written to normal chat history. MVP private mode supports **text**; private file transfer requires a separate later decision.
- Switching views keeps the private session available alongside ordinary chats. Ending the session or quitting the host closes the channel and clears the in-memory conversation.
- On iPhone, invitations appear inside the foreground app. Going to the background or losing the current Wi-Fi path ends the local private session. Nearby discovery resumes when Wi-Fi is available in the app again.
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
Personality: **Warm, precise and familiar in everyday use.**

- **Main hue:** Lavender. Reuse currently implemented brand tokens; do not revive old prototype HEX values or invent a new palette without Owner approval and accessible contrast checks. See [DESIGN_SYSTEM](DESIGN_SYSTEM.md).
- **Normal mode:** light surfaces with very subtle lavender tint, flat presentation and restrained accents.
- **Private LAN mode:** preserve independent dark theme; use explicit text/icon security states instead of relying on color as a privacy claim.
- **Interface reference:** Windows WeChat, iPhone WeChat and iPad WeChat for their respective platforms. Keep file and media organization and transfer progress as described above.


## Success definition

The product must make it obvious whether a message is **normal online**, **private LAN-only**, or part of an **AI disclosure**. No background change of transport or data access may surprise the user.
