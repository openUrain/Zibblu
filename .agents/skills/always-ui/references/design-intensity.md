# Design intensity — creative, design, improve

**Three levels of design intervention. One independent audit mode.**

These modes change **how much the Agent is allowed to redesign**, not whether it must read the product context, respect accessibility, use value presets, or follow interaction rules.

| Mode | Intent | Allowed design change | Preserve | Evidence of success |
| --- | --- | --- | --- | --- |
| **creative** | Make a recognizably different visual direction | Major visual language, composition, typography roles, surfaces, color emphasis, component treatment; reorganize layouts when relevant and authorized | Product purpose, required functionality/data, user-declared brand constraints, semantics, accessibility, security, platform operability | The resulting application shell looks meaningfully different from the baseline, not merely recolored |
| **design** | Choose and realize the right structure for the job | Information architecture, workflow layout, navigation, hierarchy, component choice and responsive arrangements | Existing brand and deliberately adopted visual system unless user authorizes changing them | A reasoned, task-fit structure with coherent visual hierarchy |
| **improve** | Make the existing design cleaner without changing its identity | Small corrections to spacing, alignment, typography hierarchy, color emphasis, button balance, card treatment and clarity | Overall layout, navigation model, brand, palette roles, typography character, component family, recognizable visual identity | Before/after are clearly the same product, with targeted defects improved |
| **check** | Audit only | No changes unless explicitly requested later | Entire implementation | Evidence-backed findings, priorities, minimal recommendations, exceptions and verification limits |

## Before working on an existing UI

1. **Inspect the baseline:** screenshot or live UI when available; inspect current layout, components, tokens, typography, theme, actions, and important states. Identify what makes the original recognizable.
2. **Read the user brief and constraints:** user-approved logo, colors, identity, data, content, business workflow, framework, target devices, accessibility and functionality remain authoritative unless explicitly authorized to change.
3. **Confirm scope:** a request to redesign one screen never grants permission to rebrand every screen or rewrite business logic.
4. **Select an intensity** from the explicit invocation; don't silently move between levels because you dislike the existing design.
5. **Apply the Skill's mandatory reference files** and reuse project tokens appropriately. Numeric choices follow [value presets](value-presets.md): reuse the existing design system by default; a creative overhaul may intentionally replace *authorized visual tokens*, but new fixed values still come from the preset categories unless a documented exception applies.

## Creative — make a new visual direction

**Use:** `$always-ui creative Redesign the existing workspace into a distinctly different visual style.`

- First describe the baseline in a short **identity snapshot**: prevailing color roles, typography character, surface treatment, composition, and motion. Don't pretend you inspected an unavailable screenshot or runtime.
- Build a new design concept tied to the product's audience and job; select a primary [archetype](design-archetypes.md) where the workflow warrants it. Consider at least two **meaningfully different** directions when exploring substantial changes, then choose one for its task fit.
- Change enough independent visual decisions to make the result **recognizably different**: composition / spacing rhythm, type hierarchy, surface geometry, palette emphasis, illustration/media language, or component treatment. This is **not** a demand to change every axis or add decorative noise.
- User-approved brand constraints remain; if those constraints leave too little room for a clearly different direction, explain the trade-off rather than violating the brand.
- Do not count merely changing the accent hue, adding a gradient, swapping icons, or applying glowing pill buttons as a successful creative redesign.
- Preserve core actions, content semantics, navigation discoverability, status meaning, contrast and focus, responsive behavior, and critical flows.
- **Deliver:** baseline summary → proposed new visual direction and rationale → meaningful differences from the baseline → key layout/components/states → implementation only if requested → compare original and new results. If baseline evidence was not supplied, call it a **new concept**, not a proven departure from the previous design.

## Design — solve structure and hierarchy

**Use:** `$always-ui design Plan the main workspace for this application.`

- Diagnose users, tasks and data objects before selecting the primary [layout archetype](design-archetypes.md). Use the existing design system when one exists.
- Make reasoned choices for navigation, working canvas, contextual panels, information hierarchy, responsive adaptation, and important states.
- When there are competing layouts, compare alternatives and decide based on work frequency, information density, screen size and interaction cost.
- **Deliver:** recommended structure and reasoning, key flow, region map or wireframe, selected token scale (if new), alternatives and risks.
- A design request **can** create a brand-new interface and can significantly improve layout, but **does not automatically authorize a wholesale stylistic rebrand** of an established application.

## Improve — refine, don't redesign

**Use:** `$always-ui improve Make the existing settings screen more polished without changing its style.`

- Start from the actual baseline and [check catalog](check-catalog.md), not a preferred template. Identify a **small number of high-impact flaws**.
- Keep recognizable composition, navigation, main components, palette and type personality. Prefer existing design tokens: adjust inconsistent gaps or visual weight using the project's own scale.
- Typical changes: quiet a competing secondary CTA; remove a genuinely redundant nested boundary; align repeated rows; make feedback clearer; fix a control mismatch; improve focus or legibility.
- Do **not** convert a sidebar to tabs, replace the chart with a new dashboard, swap the theme or font family, rewrite the component system, or add unrelated product features without explicit additional authorization.
- **Deliver:** what is already working and will remain → specific small changes and why → before/after comparison or patch when requested → what has actually been verified.
- If the problem cannot be fixed without a structural rethink, explain why and suggest `design` or `creative` as a **separate decision**; don't silently escalate.

## Check — audit without edits

**Use:** `$always-ui check Review this page for visual and interaction problems.`

- Use [check-catalog.md](check-catalog.md) with the required visual and value references to identify concrete, observable problems.
- Explain severity, location, impact, minimal correction and verification limitations. Keep valid expressive choices and purposeful exceptions.
- **Do not modify the UI**, invent functionality defects from screenshots, or silently convert a review into `improve`. A follow-up fix request is separate.

## Choosing among the modes

- **“Try a totally different look”** → `creative`.
- **“Build this product's interface / rethink how users navigate”** → `design`.
- **“Clean this up, but keep the same look”** → `improve`.
- **“Tell me what looks wrong”** → `check`.

If no mode is supplied, infer from the requested **scope of change**: a new page/plan → design, a recognizable polish pass → improve, an explicitly different visual direction → creative, an audit without edits → check. Name the assumption briefly. If instructions conflict (e.g. "change everything, keep everything unchanged"), describe the conflict and preserve explicitly constrained elements.

## Example: same Agent analytics dashboard

- **creative:** find a genuinely different layout and visual identity from the current light sidebar + KPI card grid, while retaining metrics and real tasks; **not** merely purple-to-blue.
- **design:** evaluate whether a sidebar, dense analytical canvas, comparisons and drill-downs best serve real users; build within the approved brand.
- **improve:** keep the sidebar/cards/charts and brand, but fix misplaced emphasis, inconsistent spacing, redundant borders, weak labels and CTA hierarchy.
- **check:** produce prioritized observations and recommendations only.

A different-looking screen is not automatically better. A cleaner screen is not automatically more usable. Assess both against the user's task and the documented constraints.
