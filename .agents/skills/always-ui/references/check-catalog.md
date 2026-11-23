# UI anti-pattern catalog — check and improve

In `check`, audit the **actual interface** without editing. In `improve`, use evidenced findings to guide small corrective changes within the existing visual style. This is a defect-finding guide, **not** an aesthetic template. Do not call a UI "ugly" merely because it is expressive, minimal, dense, dark, playful or unfamiliar.

For each applicable rule, record: **where → what is visible/behaves incorrectly → why it impairs task or coherence → smallest fix → how to verify**. Distinguish observed evidence from unverified inference.

## A. Visual structure and hierarchy

### C01 — Frame inside frame (duplicate boundaries)
- **Look for:** card border → inner card border → outlined button/pill → inset border, all describing the same grouping or selection.
- **Why it fails:** unnecessary visual noise, unclear hierarchy and ambiguous hit targets.
- **Prefer:** one meaningful grouping boundary, tonal contrast, whitespace, subtle separation, or an inset surface only when it has a distinct responsibility.
- **Do NOT flag:** nested outlines that distinguish separate functional zones, focus indicators, table dividers, selected objects, or required accessibility states.

### C02 — Redundant selection signals
- **Look for:** an entire selectable row outlined *plus* a selection checkbox *plus* checkmark badge *plus* selected-colored tag, all expressing the same state.
- **Prefer:** one primary selection signal with supporting hover/focus if needed; batch multi-select may legitimately need checkboxes.
- **Do NOT flag:** focus ring and selection coexisting; different signs of separate states (selected, read, verified).

### C03 — Unclear information hierarchy
- **Look for:** five equally loud buttons; timestamps as bold as titles; danger actions adjacent to the primary save action with equal emphasis.
- **Prefer:** name the user's primary task, de-emphasize metadata, show one clear local primary action, isolate destructive actions.
- **Do NOT flag:** equal emphasis for genuinely equal choices, or dense expert tools where multiple actions are frequently used.

### C04 — Excess decoration / stylistic inconsistency
- **Look for:** gradients, glows, glass, shadows, badges, giant icons, motion or illustrations without a role; competing visual motifs; each panel invents its own radius, font or color.
- **Prefer:** reuse the product's established design language; cut treatments that add no useful hierarchy, affordance, or identity.
- **Do NOT flag:** intentionally playful, gaming, editorial or expressive brands. The issue is inconsistency or interference, not decoration itself.

### C05 — Density and whitespace mismatch
- **Look for:** cramped text/click targets, stacked toolbars, huge empty hero pushing the task away, cards with one tiny label occupying half a screen.
- **Prefer:** density matching the scanning/creation workflow; group related controls and retain breathing room at meaningful boundaries.
- **Do NOT flag:** spacious reading products or dense expert data consoles purely for their density.

### C06 — Alignment, rhythm, and scale drift
- **Look for:** inconsistent columns and baseline alignment, arbitrary gaps and one-off values (such as 17px padding beside a 16px token), near-identical font sizes competing, mismatched corner/spacing systems within one UI.
- **Prefer:** reuse the product's own spacing/type/radius tokens and meaningful alignment axes; when creating a new scale, choose from [value presets](value-presets.md) rather than inventing numbers.
- **Do NOT flag:** deliberate asymmetry, a coherent established nonpreset scale, platform-native metrics, accessibility overrides, or exact asset dimensions.

### C07 — Too many navigation surfaces
- **Look for:** sidebar, top tabs, secondary subnav, floating dock, contextual tabs and card menus all offering duplicated destinations.
- **Prefer:** navigation that maps to actual scope/objects; keep persistent context only when it helps task switching.
- **Do NOT flag:** distinct global/project/document navigation levels when users genuinely need them.

### C08 — Misplaced or buried primary action
- **Look for:** Send far from composer, Save far from edited form, Approve hidden in a settings menu, next step below decorative content.
- **Prefer:** place the action near its object and at the moment of decision. Preserve clear consequences for irreversible actions.
- **Do NOT flag:** hidden rare actions that would clutter the common task.

## B. Control choice and interaction semantics

### C09 — Checkbox / switch / radio / select confusion
- **Switch:** independent on/off preference with immediate effect; do not use it for a pending form choice unless explicitly defined.
- **Checkbox:** independent/multiple choice or an acknowledgment submitted later; do not use many checkboxes for a mutually exclusive decision.
- **Radio/segmented:** mutually exclusive options with a clear selected value; use segments only if options fit.
- **Selector/combobox:** selecting values; menu = executing commands. A dropdown of actions is not a stored select field.
- **Button:** triggers an action, not persistent on/off state.
- **Why it fails:** users cannot predict what will happen, whether the value is saved, or if choices conflict.
- **Do NOT flag:** product-specific components that implement equivalent semantics with correct keyboard and state behavior.

### C10 — Overdescribed controls / unnecessary confirmations
- **Look for:** a paragraph under every self-explanatory control, confirmation dialog for harmless reversible actions, duplicate tooltips repeating visible labels.
- **Prefer:** direct labels, layout and clear states; show help on demand.
- **Exception:** errors, danger, permission scope, data exfiltration, billing and privacy disclosures must not be hidden in the name of minimalism.

### C11 — Broken click/focus regions
- **Look for:** nested buttons and links, tiny icons with visually large but inactive surroundings, hover area different from click area, focus indistinguishable from selection.
- **Prefer:** coherent click targets; visible keyboard focus; accessible name; platform-appropriate sizes.
- **Never:** remove the focus outline without a reliable visible replacement, including high contrast/forced colors.

### C12 — Selection, hover and status are indistinguishable
- **Look for:** lavender row means hovered, selected, read and active depending on screen; colored dot without meaning; "success" styled exactly like "pending".
- **Prefer:** distinct, stable semantics with supporting shape/text when status matters.
- **Do NOT flag:** subtle treatments when accessibility and state still remain clear.

### C13 — Fake or badly located feedback
- **Look for:** optimistic "saved" before persistence, disappearing failure alert, spinning forever with no exit, toast far from action with no actionable hint.
- **Prefer:** truthful pending/success/failure, progress when known, local and specific recovery, retry, accessible announcement for async status.
- **Do NOT flag:** short success confirmations after a verified result.

### C14 — Form friction and error recovery
- **Look for:** clearing inputs after failure, placeholder-only labels that vanish, errors appearing only after repeated submit, arbitrary required fields, invisible save scope.
- **Prefer:** stable labels, field-specific correction, preserved edits, explicit scope, appropriate autofill.
- **Do NOT flag:** security-sensitive clearing when policy and risk justify it.

## C. Content, responsiveness and platform

### C15 — Broken text and filename truncation
- **Look for:** file extension hidden when it is important to choose a file; ellipsis removing all distinguishing text; numbers shifting horizontally in aligned tables; unreadable long labels at zoom.
- **Prefer:** preserve meaningful basename + extension in constrained lists and keep the full name available; tabular numerals when aligned changing figures benefit.
- **Do NOT flag:** conventional end truncation when extension isn't useful, or typography with deliberate proportional numbers.

### C16 — Responsive layout is only squeezed, not adapted
- **Look for:** desktop tables shrunk to illegible mobile columns; overlapping floating buttons; touch controls behind keyboard; no navigation/back affordance.
- **Prefer:** preserve task priority; switch between panes/views or allow suitable scrolling; ensure safe touch/keyboard and readable content.
- **Do NOT flag:** intentional horizontal scrolling for genuinely tabular data with visible context.

### C17 — Motion that competes with action
- **Look for:** panels that spring excessively, drawers that queue animations on rapid reversals, expanding areas that clip async content, hidden controls still tabbable.
- **Prefer:** interruptible transitions, native content sizing where reliable, synchronized state, reduced motion support.
- **Do NOT flag:** deliberate animation in expressive products if it remains usable and accessible.

### C18 — Desktop host interaction conflicts
- **Look for:** custom titlebar draggable across buttons, text or inputs; no drag region on empty titlebar; scroll surfaces that grab a window instead.
- **Prefer:** host-supported drag on noninteractive surfaces and test resize, double-click and controls.
- **Do NOT flag:** ordinary browser webpages that have no native host window drag API.

### C19 — Accessibility hidden by aesthetic cleanup
- **Look for:** contrast too weak, focus indicator removed, color-only error/success, inaccessible keyboard controls, icons with no meaningful name.
- **Prefer:** visual polish that preserves semantics and keyboard/screen-reader/high-contrast operability.
- **Do NOT claim:** measured contrast without sampling actual rendered colors and relevant text sizes.

### C20 — Component behavior inconsistent across screens
- **Look for:** same icon meaning different things, different save behavior for identical settings, arbitrary placement of cancel/delete, inconsistent modal/back patterns.
- **Prefer:** consistent role, semantics and feedback, adapted to each platform's conventions.
- **Do NOT flag:** intentional differences between touch and keyboard experiences.

## D. Visual discipline: four cross-mode checks

These are actionable checks from [visual discipline](visual-discipline.md), not taste-based bans. Compare the **application shell** to its own brand/semantics; do not count colorful customer artwork or embedded previews as decorative UI.

### C21 — Color restraint (V01)
- **Look for:** competing high-saturation accents on routine controls, state colors used as decoration, or one color doing conflicting jobs across the screen.
- **Prefer:** purposeful brand/action, semantic status, focus/selection, and neutral roles; quiet nonessential accents without removing meaning.
- **Do NOT flag:** colorful previews, expressive brands, categorical visualizations, or distinct status colors when they preserve clarity.

### C22 — Button hierarchy (V02)
- **Look for:** multiple equally loud CTAs in one decision zone, glowing pill buttons for routine tools, or icons beside every label without an identifiable benefit.
- **Prefer:** one clearly prioritized next step per **local action group**, quieter secondary controls, icons that disambiguate rather than decorate.
- **Do NOT flag:** independently scoped primary actions in different parts of a page, genuinely equivalent choices, or product-consistent pill/icon buttons.

### C23 — Decoration justification (V03)
- **Look for:** gradients, glows, glass, shadows, badges and layered outlines with no role in hierarchy, interaction, grouping or deliberate identity.
- **Prefer:** fewer effects with identifiable purpose. Ask which information or affordance disappears if an effect is removed; simplify only the redundant parts.
- **Do NOT flag:** legitimate brand expression, meaningful depth/focus cues, or artwork in a file/design preview. Decorative restraint must not remove keyboard-focus visibility.

### C24 — Component economy (V04)
- **Look for:** every repeated file, comment or metric put inside a separate rounded card, sometimes with another card inside it, despite all items serving one list.
- **Prefer:** list/row structure, spacing, shared panels or subtle surface differentiation where they convey the same information more clearly.
- **Do NOT flag:** cards needed for separate actions, independent objects, selection/drag affordances, accessible targets or meaningful grouping.


## E. State transitions and layout stability

### C25 — Unnecessary layout shift across states (stable geometry)
- **Look for:** a queued-message clock appearing below a bubble then disappearing and moving adjacent rows; a save/error indicator changing the button's horizontal position; loading placeholders much smaller than the final preview or profile; status labels or icons forcing a toolbar to resize or a search field to shrink before space is constrained.
- **Prefer:** preserve the principal control/row geometry, alignment baselines, and neighboring positions across pending, loading, success, error, hover, selected and disabled states when the underlying content is unchanged. Reserve a consistent status slot, keep action alignment stable, and size placeholders from known final dimensions or aspect ratio. Reuse existing product tokens; do not introduce an empty decorative container just to satisfy this rule.
- **Verify:** compare the same viewport and content across states with before/during/after screenshots or interaction recordings; inspect element bounds and neighboring displacement when the runtime is available. A static screenshot alone cannot prove a state-transition jump. Distinguish a real shift from a color/opacity change, and require truthful, accessible progress/error announcements.
- **Do NOT flag:** real content growth, user-initiated expansion/collapse, responsive reflow due to genuinely insufficient space, accessibility text scaling or focus accommodation, and deliberate product-approved motion that does not obscure content or break interaction. Do not enforce zero pixel movement universally or hide failure feedback to keep geometry stable.

## Triage rubric

| Class | Definition | What to do |
| --- | --- | --- |
| **Defect (P1/P2)** | Misleading, inaccessible, broken or dangerous behavior | Prioritize by severity and reproduction |
| **Design issue (P2/P3)** | Clear interference with hierarchy, scanability or workflow, supported by evidence | Suggest targeted improvement and comparison |
| **Polish (optional)** | Aesthetic judgment without clear task impairment | Label as preference, offer alternatives, do not demand changes |
| **Not applicable** | Rule has no bearing on this surface/platform | Omit from findings |

### Recommended report format

| Impact | Location | Observation (evidence) | Consequence | Minimal recommendation | Verification |
| --- | --- | --- | --- | --- | --- |
| P2 · design issue | Conversation list / selected row | Row outline and checkmark duplicate selection state | Higher noise, weaker grouping | Keep a single selected treatment; retain keyboard focus | Screenshot comparison pending |

Conclude with **Keep as-is:** what the current product already does well. Prefer 3–7 high-value findings to 30 unsupported complaints. Never invent pixel measurements, usage data or runtime checks.
