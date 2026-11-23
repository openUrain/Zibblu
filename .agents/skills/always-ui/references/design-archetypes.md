# Layout archetypes — creative and design

**Use cases, not universal templates.** In `creative` and `design`, select by what users spend most of their time doing. `improve` should preserve an established layout unless the user explicitly requests a larger change. "SaaS" is a delivery/business model: a SaaS product may be a chat app, analytics workspace, editor, or operations console. The labels below describe dominant workflows, not mutually exclusive industries.

## Diagnose before choosing

Ask of the actual product, or infer from its brief: Who is the primary user? What do they do in the first 10 seconds? What object is manipulated most (message, item, document, metric, record)? Do they scan many objects, edit one object, or compare multiple objects? Are actions frequent, real-time, high-stakes, or collaborative? Is this desktop, mobile, tablet, or a responsive web app? Which parts must remain visible?

Choose **one primary archetype**, optionally **one secondary archetype**. If a product spans multiple jobs, route between them without combining all their chrome on every screen.

| # | Archetype | Dominant job | Likely main surface |
| --- | --- | --- | --- |
| 01 | SaaS / multi-tool workspace | Resume work across modules | Active work + prioritized routes |
| 02 | Messaging / chat | Follow conversations and reply | Conversation list + chat canvas |
| 03 | Project / task management | Organize work items and progress | List, board or timeline |
| 04 | Analytics / monitoring | Compare data, detect and explain changes | Filters + chart/table + drilldown |
| 05 | E-commerce / marketplace | Discover and evaluate items, transact | Catalog/search + product detail |
| 06 | Editor / knowledge base | Create, structure and retrieve content | Focused editable canvas |
| 07 | Social / community | Follow people, posts and discussions | Feed, thread or community space |
| 08 | Media / file library | Find, preview and organize assets | Search/browse grid/list + viewer |
| 09 | Finance / transactions | Review balances, move or reconcile value | Accounts/ledger + action detail |
| 10 | Admin / operations console | Inspect records, configure, respond to issues | Record table + contextual actions |
| 11 | Booking / services | Select a service, time, resource and commit | Guided selection + confirmation |

## 01 — SaaS / multi-tool workspace

**Choose when:** users genuinely switch between multiple tools and need a central view of ongoing work.

- **Desktop:** global navigation for actual destinations; active items, recently visited objects and action-required exceptions; relevant summaries only.
- **Mobile:** resume current work and one primary action first; secondary modules via compact navigation, not a miniature KPI grid.
- **Interactions/states:** workspace switching, personal/team scope, permission-aware navigation, search when scale justifies it, first-use onboarding.
- **Avoid:** a home dashboard full of arbitrary metrics; repeating the same actions in sidebar, header and cards; using a sidebar just because the product is sold as SaaS.
- **Consider instead:** open directly into the most frequent tool, with a lightweight global switcher.

## 02 — Messaging / chat

**Choose when:** conversational reading/writing is the product's core loop.

- **Desktop:** list of conversations with unread/search context; large message canvas and anchored composer; optional details/thread panel only when requested or the workflow warrants it.
- **Mobile:** one conversation at a time, clear list ↔ conversation navigation, keyboard-safe composer, reliable attachment entry.
- **Interactions/states:** read/unread, send pending/failure/retry, offline, typing, attachment transfer, message actions and access/privacy disclosure.
- **Avoid:** nested cards around every message; chat pushed aside by dashboard furniture; duplicate read/selection icons and toolbars.
- **Variations:** service/support inboxes may need assignment and queue filters; small-group messaging can be lighter.

## 03 — Project / task management

**Choose when:** items have status, owner, deadline, dependencies or stages.

- **Desktop:** list for comparison, board for moving stages, timeline/calendar for time dependencies; shared filters/grouping and item detail panel.
- **Mobile:** focused task list/detail with direct update controls; don't force unreadable full-width boards into phone screens.
- **Interactions/states:** batch selection, inline edits, optimistic update rollback, filter persistence, drag-and-drop alternatives.
- **Avoid:** massive task cards showing every field; default Kanban when a table or timeline fits better; burying status changes in modal stacks.
- **Consider:** one default view selected by the dominant job; additional views optional.

## 04 — Analytics / monitoring

**Choose when:** users need evidence for decisions, compare values or detect anomalies.

- **Desktop:** scope/period and filters near the analysis area; one meaningful overview; charts/tables with clear units, baselines and drilldown.
- **Mobile:** summarize most important trend and alert; separate deep exploration instead of shrinking every desktop panel.
- **Interactions/states:** timestamps, data freshness, denominator/unit clarity, selected time range, loading/partial/error states, accessible table alternatives.
- **Avoid:** decorative KPI cards, unlabeled axes, misleading scales, color-only alert severity, mixing incomparable periods.
- **Variation:** operations monitoring prioritizes live anomalies; analysis tools prioritize query and exploration.

## 05 — E-commerce / marketplace

**Choose when:** users search, filter, compare and decide what to buy or book from a catalog.

- **Desktop:** prominent discovery entry, filters near results, informative product cards, detailed product pages and a visible purchase/next-step flow.
- **Mobile:** search, thumb-reachable filters and product details; high-value selection or purchase action remains obvious.
- **Interactions/states:** variants, availability, shipping/fees, comparison, saved items, cart, out-of-stock and payment failure.
- **Avoid:** product cards that hide price/availability; massive promotional hero that prevents discovery; checkout steps with hidden costs.
- **Variation:** specialized B2B catalogs may prioritize tabular specification comparison instead of imagery.

## 06 — Editor / knowledge base

**Choose when:** producing, revising or retrieving documents/content is the main activity.

- **Desktop:** focused central canvas; document/navigation tree when needed; contextual formatting and properties, not permanent toolbar sprawl.
- **Mobile:** reading and lightweight editing priorities; progressively reveal complex editing tools.
- **Interactions/states:** autosave, collaboration/cursor presence, versions/history, find, conflict/error recovery, empty document.
- **Avoid:** crowded side panels competing with the writing surface; settings dialogs for basic editing; unclear save state.
- **Variation:** an IDE-like tool may favor multi-pane context; a note app may favor near-full-screen writing.

## 07 — Social / community

**Choose when:** users explore posts, follow people, and participate in discussions or groups.

- **Desktop:** content feed or focused community/channel; readable threads; discovery and creation actions easy to reach without dominating content.
- **Mobile:** comfortable feed scanning, clear interaction targets and comment/reply context, safe media handling.
- **Interactions/states:** post composer, thread hierarchy, reactions and moderation, privacy/visibility, content loading and reporting.
- **Avoid:** treating every post as a dashboard tile, overloading each item with action icons, ambiguous public/private visibility.
- **Variation:** forum communities prioritize threads/search; creator platforms may prioritize a gallery.

## 08 — Media / file library

**Choose when:** users browse many assets, preview them, and organize or transfer files.

- **Desktop:** grid for visually distinguishable assets, list/table for names, metadata, and operations; viewer/pane for inspection and batch commands when selected.
- **Mobile:** quick search, preview, upload/download progress and straightforward sharing; contextual actions instead of persistent full toolbars.
- **Interactions/states:** file types, preview fallback, sort/filter, bulk selection, upload progress/failure, rename, permissions.
- **Avoid:** identical thumbnails when filename/type is primary; clipping essential extensions; nested selection checkboxes and decorative outlines.
- **Variation:** media streaming privileges playback; document management privileges metadata and permissions.

## 09 — Finance / transactions

**Choose when:** users need precise balances, transaction histories, forecasts, approvals or transfers.

- **Desktop:** account/context selector, ledger/table, clearly separated primary transaction action and detailed records; numbers align consistently.
- **Mobile:** essential balance/action/tracking with progressively disclosed details; confirmation states remain explicit.
- **Interactions/states:** currency/unit, pending vs settled, timestamps, permissions, validation, confirmation, transaction failure/recovery and audit trail.
- **Avoid:** hiding amounts and fees behind ambiguous labels; confusing credit/debit conventions; color-only gain/loss; false success.
- **Variation:** personal budgeting uses narrative trends; enterprise reconciliation uses dense tabular comparison.

## 10 — Admin / operations console

**Choose when:** users inspect many records, troubleshoot exceptions, configure systems or moderate content.

- **Desktop:** search/filterable record list or table, selected record context, grouped actions, status and permissions close to affected objects.
- **Mobile:** high-priority alerts and simple approval/triage; defer very dense configuration where necessary.
- **Interactions/states:** roles/permissions, audit logs, batch actions, dangerous changes, disabled/locked records, long-running jobs, retry.
- **Avoid:** nesting the settings panel behind repeated cards/modals, mixing destructive actions with primary ones, ambiguous bulk selection.
- **Variation:** an incident response console values alerts and real-time status; a settings console values clear grouping and persistence.

## 11 — Booking / services

**Choose when:** the primary journey is choosing a resource, a date/time, options, then confirming a reservation.

- **Desktop:** overview/availability plus contextual detail; stepwise selection when decisions depend on each other, concise persistent summary.
- **Mobile:** task-focused flow with clear progress/back navigation and a readable reservation summary before final confirmation.
- **Interactions/states:** availability changing, time zones, capacity, selected options, price changes, validation, confirmed vs pending, modification/cancellation.
- **Avoid:** fake availability, hidden final fees, prematurely clearing selections, excessive steps or an ambiguous confirm button.
- **Variation:** frequent repeat bookings may favor a shortcut; first-time complex services need guided choices.

## Hybrid routing examples

- **SaaS chat platform:** primary Messaging, secondary Admin or Media. The conversation is the main canvas, not a KPI dashboard.
- **Enterprise project analytics:** primary Analytics when users ask "why did performance change?"; primary Project Management when users mostly move tasks.
- **Financial marketplace:** Commerce for searching instruments/products, Finance for portfolio execution and holdings. Separate high-stakes flows.
- **Community knowledge platform:** Social if people/threads dominate; Editor if durable documents and writing dominate.

## Decision output template

1. **Diagnosis:** main user, top job, object, density, input platform, unknowns.
2. **Choice:** primary archetype + optional secondary, with why it fits.
3. **Structure:** main regions in priority order; desktop and mobile adaptation; one alternative considered.
4. **States:** first use, selected, empty/loading/error, permissions, search/filters if needed.
5. **Numeric choices:** inspect the product's existing tokens first. If missing, choose spacing, type, radius, borders, control sizes, elevation and motion exclusively from the [value preset tables](value-presets.md); state any justified exception without arbitrary one-off values.
6. **Visual discipline:** define purposeful color roles, local primary-vs-secondary actions, which decorative effects earn their place, and whether repeated items need cards; use [visual discipline](visual-discipline.md) without applying a preset aesthetic.
7. **Anti-patterns:** 2–4 task-specific things to avoid (not blanket style prohibitions).
8. **Open decisions:** only the decisions materially affecting the implementation.

Patterns can be deliberately broken for good reasons. The user and product context decide the design; this catalog helps an Agent reason, not obey a formula.
