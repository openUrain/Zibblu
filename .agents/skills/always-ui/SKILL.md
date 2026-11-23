---
name: always-ui
description: UI/UX Agent Skill with three levels of design change and one independent audit. Use "creative" for a distinctly different visual direction, "design" for task-led layout and hierarchy, "improve" for small changes preserving the current style, and "check" for an evidence-based UI audit. Read project guidance and required references before working.
license: MIT
argument-hint: "[creative|design|improve|check] [product, page, or component]"
---

# Always UI

**Choose how far to change the design. Keep the judgment. Guardrails, not taste.**

Always UI offers three **design intensities**—`creative`, `design`, `improve`—plus an independent **audit** mode, `check`. An Agent must not escalate from minor polish to a redesign without permission.

## Mode selection

- Explicit invocations: `$always-ui creative`, `$always-ui design`, `$always-ui improve`, `$always-ui check`.
- **creative — large redesign:** intentionally explore a recognizably different visual direction from an inspected baseline, while preserving authorized product behavior and declared brand constraints.
- **design — standard design:** plan or create task-fit layout, navigation, components and hierarchy; maintain the established brand by default.
- **improve — small polish:** retain the current layout, branding and recognizable component language; correct localized visual and interaction problems.
- **check — audit only:** review a UI and report issues with evidence; do not change implementation unless separately requested.
- When no mode is specified, infer the intended **degree of change**: clearly different look → creative; new interface or structural planning → design; existing-UI refinements → improve; audit/critique without edits → check. State the assumption. Do not default every existing UI request to a redesign.
- When any design intensity and `check` are both requested, perform a clearly separate **design pass → check pass**. `check` does not count as a fourth design intensity.

## Reference-first workflow — mandatory

**Read the relevant Always UI reference files before deciding, generating, changing or reviewing UI.** These files are the Skill's instructions, **not optional inspiration**. Do not skip them and improvise from memory. Follow the documented classifications, checks and numeric tables rather than inventing your own rules.

| Mode | Required references, read first | What to take from them |
| --- | --- | --- |
| `creative` | [design-intensity.md](references/design-intensity.md), [design-archetypes.md](references/design-archetypes.md), [value-presets.md](references/value-presets.md), [visual-discipline.md](references/visual-discipline.md) | Inspect the old style; explore a genuinely different direction while respecting product requirements and numerical discipline |
| `design` | [design-intensity.md](references/design-intensity.md), [design-archetypes.md](references/design-archetypes.md), [value-presets.md](references/value-presets.md), [visual-discipline.md](references/visual-discipline.md) | Diagnose workflow, select a layout, reuse project tokens and plan consistent hierarchy |
| `improve` | [design-intensity.md](references/design-intensity.md), [check-catalog.md](references/check-catalog.md), [value-presets.md](references/value-presets.md), [visual-discipline.md](references/visual-discipline.md) | Find the most valuable small fixes, preserve the baseline's appearance and use existing tokens |
| `check` | [check-catalog.md](references/check-catalog.md), [visual-discipline.md](references/visual-discipline.md), [value-presets.md](references/value-presets.md) | Assess documented anti-patterns and numeric drift **against project tokens first** |
| `any design mode + check` | The selected design mode's files **and** the check files | Complete separate passes without silently changing the authorized design intensity |

**Also read the project's design system, existing components and tokens.** Precedence is: user requirements, safety/accessibility/platform constraints and established product design decisions → project tokens and interaction contracts → Always UI's applicable guidelines and fallback numeric tables. If sources disagree, disclose the conflict and preserve the higher-priority requirement. Do not substitute a new aesthetic for the product's identity **except where creative explicitly authorizes a new visual direction**; still honor the user's non-negotiable brand, accessibility and product constraints.

- **Before implementation:** use the selected layout guidance and documented scale; inspect the project's current tokens before writing any new fixed value.
- **Before review:** consult the actual check catalog and report only issues grounded in visible UI/code and applicable rules. Do not use the fallback table to criticize a consistent existing scale.
- **Technical details:** when implementing or assessing focus rings, expansion, filename rendering, state-driven geometry or native drag regions, **read** [implementation-notes.md](references/implementation-notes.md) before deciding how to handle that behavior.
- **Optional style, by design:** [visual-guidance.md](references/visual-guidance.md) is **only** for products that explicitly choose its restrained/tonal aesthetic; it must not become the default look.
- If required files cannot be accessed, **say which references were unavailable** and do not claim a fully Skill-guided design/check. Request or retrieve the files instead of silently replacing their guidance with guesswork.

## Shared principles

1. Read the user's requirements and the product's existing design system first. Preserve platform conventions, interaction contracts, accessibility and security. Preserve the existing visual style in `design` and `improve`; `creative` may change the **explicitly authorized** stylistic parts while keeping stated brand constraints.
2. Look at actual workflows: user role, top tasks, primary data objects, density, frequency, screen size, touch/mouse/keyboard, and how users recover from errors. State assumptions when evidence is unavailable.
3. **Patterns are options, not templates.** SaaS does not automatically mean a dashboard; chat does not automatically need three columns. Do not force rounded cards, pastel gradients, tiny text, oversized whitespace, or any other house style.
4. Distinguish an **accessibility/semantic defect**, a **task-level design issue**, and a **subjective aesthetic preference**. Evidence beats taste.
5. Restrict changes to the authorized scope. Design-only tasks produce plans; implementation and cleanup require their own authorization.
6. Do not claim to have viewed screenshots, run UI tests, measured contrast or tested touch/keyboard unless actually done.
7. **Stable geometry across states:** for the same control or repeated row, prefer stable dimensions, alignment and neighboring positions as pending/loading/success/error/selection states change. Do not let a disappearing status icon or a loading placeholder cause avoidable layout jumps. Check [C25](references/check-catalog.md) and [implementation notes](references/implementation-notes.md) for evidence and techniques. Real content growth, deliberate disclosure, responsive layout and accessibility-driven changes may legitimately reflow.

## Four visual discipline rules — all modes

Apply [references/visual-discipline.md](references/visual-discipline.md) as a **context-aware decision aid**, not a universal minimalist style:

1. **Color restraint:** give brand, semantic status, focus/selection and decorative colors distinct jobs; do not make every element compete for attention. Keep content/artwork colors separate from the surrounding application shell.
2. **Button hierarchy:** establish a clear primary action **within each local action group**; make secondary actions quieter when appropriate. Avoid defaulting every button to a glowing pill with an icon.
3. **Decoration justification:** gradients, glows, shadows, badges and extra outlines should support meaning, interaction or an intentional brand direction. Reduce redundant effects; do not ban expressive aesthetics.
4. **Component economy:** use a card or container when it creates a useful boundary, selection target or independent object. Otherwise consider a list, spacing, or surface hierarchy before nesting more cards.

**Do not** enforce a fixed color count, one-CTA-per-screen limit, a blanket gradient/card ban, or changes to client-supplied artwork. An observation about aesthetic taste is not automatically a defect.

## Numeric presets — design must choose from tables

When selecting fixed UI design values, follow [references/value-presets.md](references/value-presets.md) **before writing styles**. Read it in `creative`, `design`, `improve`, and in `check` when auditing numeric drift.

- **First:** inspect the existing project's design tokens, UI library scales and platform metrics. `design` and `improve` reuse them. In an explicitly authorized `creative` overhaul, existing tokens form the baseline; newly chosen or replaced visual token values must still use the documented tables or justified exceptions, not random numbers.
- **If missing:** choose values from the category-specific tables for spacing/sizing, type/weight/line-height, radius/borders, icons/control heights, alpha/color stops, elevation, motion, breakpoints and layers. Give reused choices semantic tokens.
- **Do not improvise:** `17px` padding, `23px` gap, new shadow recipes, arbitrary Tailwind values or other unlisted fixed design numbers because they "look right." Inspect generated CSS/styles for numeric drift before completing the task.
- **Exceptions:** intrinsic/fluid sizes, asset geometry, user-specified sizes, accessibility/platform requirements and content-driven constraints may require values outside the tables. State the reason, and keep the exception deliberate rather than silently creating another scale.
- Do not turn these presets into a global aesthetic or a numeric limit on brand colors. The rules constrain *measurements*, not product personality.

## Mode: creative

1. **Inspect the old look before creating a new one.** Identify the baseline's layout, palette roles, typography, surface treatment and component vocabulary. If no baseline is available, describe your work as a **new concept**, not a verified departure.
2. **Read and follow** [design-intensity.md](references/design-intensity.md), [design-archetypes.md](references/design-archetypes.md), [value-presets.md](references/value-presets.md) and [visual-discipline.md](references/visual-discipline.md). Let the real product workflow determine what must remain usable.
3. **Explore genuinely different directions.** For a substantial redesign, compare at least two meaningful concepts, select one, and make multiple coherent changes beyond an accent-color swap or decorative effects. Do not equate creativity with more gradients, glowing pills or visual noise.
4. **Respect invariants:** preserve user-required branding, product function, information truth, security/privacy, accessibility and platform operability. If a requirement prevents a striking difference, describe the constraint rather than breaking it.
5. **Choose consistent tokens** using the numeric tables. You may deliberately replace authorized style tokens, but never inject unexplained fixed values or a competing token scale.
6. **Deliver:** baseline identity → new direction and why it fits → concrete differences → key screens/states → comparison against the original (when observable). Implement only if asked; do not redesign unrelated screens.

## Mode: design

1. **Classify the actual job, not the marketing category.** Identify the main task (communicate, browse, analyze, create, transact, organize, operate) and the primary object (message, project, metric, document, product, record, etc.).
2. **Choose a primary archetype** and, if needed, one supporting archetype from [references/design-archetypes.md](references/design-archetypes.md). If a product is hybrid, explain which activity owns the main canvas; do not stack every pattern's navigation.
3. **Derive the hierarchy:** the thing users need to see first, primary and secondary navigation, workspace, contextual details, tools, and next action. Include responsive and input-method differences.
4. **Consider alternatives:** for substantial layout decisions compare at least two viable structures (e.g. list/detail vs board, single canvas vs multiple panes) and choose based on workflow, density, platform and trade-offs.
5. **Plan key states:** first use/empty, search/filter, loading, offline, permission, failure, selection and destructive actions where applicable. Avoid decorating before interaction paths make sense.
6. **Choose numeric tokens from tables:** audit the existing token system; if absent, pick fixed values only from [references/value-presets.md](references/value-presets.md). Apply scale consistently; record justified exceptions instead of making up ad-hoc numbers.
7. **Run the visual discipline pass:** allocate color roles, establish local button hierarchy, justify prominent decoration, and decide which repeated elements genuinely need cards. Follow [references/visual-discipline.md](references/visual-discipline.md).
8. **Deliver:** product diagnosis; chosen archetype; annotated desktop/mobile region map or text wireframe; key interactions and states; chosen token scales (when newly defined); alternative rejected with reason; no-go patterns; assumptions and what needs approval.
9. Only implement when requested. Keep explicit existing product decisions; do not relabel a design preference as a requirement.

## Mode: improve

1. **Inspect the existing UI and its approved design system** before changing anything. Read [design-intensity.md](references/design-intensity.md), [check-catalog.md](references/check-catalog.md), [value-presets.md](references/value-presets.md) and [visual-discipline.md](references/visual-discipline.md).
2. **Preserve the recognizable product:** navigation, overall composition, palette roles, typography personality and component family stay in place. The user asked for refinement, **not** a new style.
3. **Choose a few high-impact corrections:** inconsistent spacing, crowded controls, redundant framing, weak information hierarchy, too many loud buttons, unclear state feedback, subtle accessibility improvements, avoidable shifts between transient component states.
4. **Reuse the existing tokens** and components; any new fixed value must follow the preset-selection rules. Avoid replacing whole page structures, rebranding, global component rewrites or introducing new features.
5. **Deliver:** what is kept → each small change and its reason → before/after evidence where available → verification status. If structural redesign is necessary, explain and propose switching modes; **do not escalate automatically**.
6. Implement only within the user's authorized scope.

## Mode: check

1. **Observe** the actual UI and any available screenshots/code/design system. Record screen, platform, viewport and confidence. Without runtime or images, mark visual and interaction conclusions as hypotheses.
2. Apply the relevant checks in [references/check-catalog.md](references/check-catalog.md): boundary duplication (frame inside frame), selection over-signaling, incorrect checkboxes/switches/radios, poor hierarchy, excessive decoration, density, inconsistency, feedback/focus/motion, state-to-state geometry stability and device interaction. Specifically inspect **color restraint, button hierarchy, decoration justification and component economy** using [references/visual-discipline.md](references/visual-discipline.md). Where numeric inconsistency matters, compare values against the **existing project tokens** first, then the fallback [value presets](references/value-presets.md) only for newly introduced scales. Do not treat a coherent established scale as a defect simply because its numbers differ.
3. When state behavior is available, compare the same component **before/during/after** loading, failure or completion; report unexpected shifts with screenshots or recordings. If only a static screenshot is available, ask for reproduction instead of claiming a dynamic defect.
4. **Explain the user's problem, not just the style violation.** A decorative border is not automatically wrong; it is wrong when it muddies grouping, click targets, or hierarchy. An unusual layout is not wrong solely for being unusual.
5. Classify findings: **Defect** (broken, misleading, inaccessible), **High-confidence design issue** (material task friction), or **Optional polish** (taste). Rank by impact and reversibility, not aesthetic fashion.
6. Recommend the **smallest corrective action**, naming what should remain unchanged. Provide observable acceptance checks and distinguish tested from untested behavior.
7. Only edit code if the user requested implementation or fixes.

**Check report format:** `severity | location | observed issue | user impact | minimal change | verification status`. Keep findings specific; avoid a laundry list of style opinions.

## Before you finish

- **`creative`:** confirm a meaningful difference from the inspected baseline, without loss of required functionality. If no baseline was available, disclose that. **`design`:** confirm the selected layout and numeric decisions use required references. **`improve`:** confirm the original visual identity and navigation remain intact and changes are localized.
- **`check`:** confirm the reported issues map to the documented catalog or evidence-backed design principles; explain valid exceptions rather than manufacturing complaints.
- **All modes:** honor explicit user constraints, distinguish verified behavior from assumptions, and do not claim to have consulted unread or inaccessible files. For `creative`, preserving project identity means preserving **declared brand constraints**, not necessarily the old styling.
