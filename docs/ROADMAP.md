# Roadmap and minimum acceptance

**Approach:** three complete functional modules, in order. Implement only what is necessary for each outcome. No calendar promises, premature microservices or test matrices.

> **文档定位（2026-11-20）**：M1/M2/M3 下方是功能交付范围与最小验收基线，**不是尚未实现事项的实时任务板**。Windows 已有公开 Release 和后续迭代；实际当前功能、已知限制与部署方式以 [PRODUCT](PRODUCT.md)、[SECURITY](SECURITY.md)、[ONLINE_DEPLOYMENT](ONLINE_DEPLOYMENT.md)、GitHub Issues/PRs 为准。UI 分模块整改见 [Issue #59](https://github.com/openUrain/Zibblu/issues/59)，设计/审查使用 [Always UI](DESIGN_SYSTEM.md)。

## M1 — Normal Chat & Files

**Deliverable:** a usable desktop messenger with standard online direct/group communication and strong file handling.

Scope:
- Desktop shell, login/identity and contacts needed for real chats.
- Message sending, receiving, persistent history and basic offline delivery.
- Basic group conversation.
- Send file/folder; progress, pause/resume, integrity check, clear failure state.
- Per-chat file history plus a minimal global Files view.

**Done when:** two desktop clients can exchange normal messages, receive a queued message after reconnecting, and transfer/resume a nontrivial file that matches the source. File history is findable from the UI.

**Minimum test at completion:** one end-to-end normal conversation, one offline/reconnect case, one interrupted/resumed file with integrity check. Add focused rejection coverage for an unauthorized file request. No broad stress suite yet.

## M2 — LAN Private Chat

**Deliverable:** explicit temporary **text** sessions on the same reachable LAN, separate from normal chats.

Scope:
- Ghost control in direct and group chats, in-app full-window invitations, lower-right prompts when minimized on Windows, QR rooms, account-wide approval, and authenticated local transport.
- Clear verified/private/failed states.
- Memory-scoped conversation and session keys.
- End/disconnect/timeout cleanup and view-independent sessions. Support routed internal subnets under an explicit company LAN policy.
- No server transcripts, no cloud AI, no private-content notifications or indexing.

**Done when:** two LAN peers start by consent, exchange text, close and see no persisted private history; out-of-LAN/unreachable peers cannot silently send via cloud.

**Minimum test at completion:** direct invitation, staggered group and same-account device joins, QR expiry, cross-VLAN discovery with approved routing, and denied or unreachable peers. Verify no private transcript after restart and that group QR users see only post-join normal history.

## M3 — AI Assistants

**Deliverable:** one-shot `@custom-name` AI calls with explicit context limits and read-only file access.

Scope:
- User-configured AI providers and custom mention names.
- Before every request, choose a bounded message range or selection; optionally choose exact files.
- Preview what data goes to a remote provider, then send that snapshot only.
- Show response in chat according to an explicit visibility choice.
- No file/chat mutation, arbitrary file system access, background access or private-mode cloud context.

**Done when:** a user can trigger a named AI, authorize only a particular context and get a reply; expanding scope or writing files is impossible through the AI interface.

**Minimum test at completion:** allowed scoped request; out-of-scope data not submitted; write attempt denied; cloud AI blocked in private mode.

## Long-term product and commercialization (direction, not committed milestones)

**Owner decision (2026-11-23):** Zibblu adopts **one product, free personal use, paid advanced organizational capabilities** as its long-term direction. Do not split the client into separate consumer and enterprise messengers. The same core chat, file and protocol experience should work across personal, team and enterprise deployments.

**Positioning:** a simple, local-first messenger with reliable file collaboration, opt-in private LAN sessions and permission-bounded AI. Small technical/project teams are an initial market to validate, not a restriction on free individual use. Avoid becoming a general office suite or copying attendance, approvals, CRM and other unrelated modules.

| Layer | Intended value | Commercial direction |
| --- | --- | --- |
| **Community / personal** | Ordinary direct/group chat, file sharing and resume, local history, private LAN chat and personally configured read-only AI | Core product remains free; community self-hosting remains an option |
| **Team Cloud** | Official hosted service, reduced deployment/maintenance burden and organization-oriented collaboration features | Potential subscription; hosting storage/traffic can have reasonable published quotas, without crippling core chat |
| **Enterprise** | Organization administration, role/policy controls, SSO, managed model configuration, backup/retention/audit capabilities and professional deployment/support | Potential commercial licensing, support or service contracts for validated needs |

These are **potential capability boundaries, not a current pricing table, feature-completion claim or promise to ship three editions**. Keep one maintainable codebase and add organization features only when backed by user evidence. Core messaging reliability, essential security updates and existing basic privacy controls must not become paywall incentives.

**Non-negotiable boundaries:**
- M1 ordinary server-routed messages are not end-to-end encrypted; commercial materials must describe their actual privacy model. An enterprise's self-hosted server can access content during online delivery as documented in [SECURITY](SECURITY.md).
- M2 private LAN text stays authenticated, consent-based and LAN-only with no cloud/AI fallback. Organization policy may enable or disable this mode, **never add a silent administrator backdoor to read private sessions**. Any enterprise retention or audit behavior must be transparent and must not silently change M2's security contract.
- M3 AI access remains explicitly permission-bounded and read-only. Enterprise model connections or knowledge retrieval must follow the same authorization rules, not gain implicit cross-chat or file access.
- Before monetization, review the existing public source licenses, third-party dependencies, ownership and commercial module boundary; do not retroactively assume public licensed code can be made proprietary. Account/data governance, support and applicable compliance require separate validation.

**Validation order, not dates:** (1) make the free core trustworthy and pleasant for daily use, including mobile basics and dependable file/message handling; (2) observe sustained use by real small teams, especially conversation-to-file and AI knowledge workflows; (3) test willingness to pay for hosting, organization control and support; (4) scope enterprise capabilities and billing only after demand and operating costs are understood. Do not open implementation Issues for billing, subscriptions, enterprise administration or a second client solely because this direction is recorded.

## Issue / PR discipline

- One module per implementation PR unless a clearly necessary dependency is separated first.
- Explain completed behavior and explicit omissions; avoid claiming future scope is implemented.
- At module completion, run the above minimal checks and report actual commands/results.
- Review against the module acceptance criteria and security invariants. Fix substantive findings, then merge only per repository owner's instruction.

**Platform status / future scope:** Windows is the delivered client baseline. An **unsigned iPhone/iOS preview build exists for testing**; this is not an App Store release or proof of real-device acceptance. iPad adaptations require platform-specific verification. Public mobile distribution and wider production support are separate follow-up work, not “mobile never started.” Calls/video, social feed, public bot marketplace, elaborate plugin systems, autonomous AI agents, private attachment exchange and speculative multi-cloud infrastructure are not committed v1 deliverables.
