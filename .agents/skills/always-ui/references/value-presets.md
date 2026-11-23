# Value presets — choose from the table, don't invent numbers

This is the **numeric decision table for `$always-ui creative`, `$always-ui design`, and `$always-ui improve`**. It applies when selecting new visual tokens or adding values to a product without matching tokens. In `check`, it is a reference for spotting inconsistent new values, **not** a reason to invalidate the established design system.

**Rule:** Do not choose arbitrary fixed UI measurements. If a new design needs a literal spacing, font size, weight, radius, border width, icon/control size, opacity, elevation, motion duration, breakpoint or z-layer, **pick a value from its category below**. Reuse that choice through a named token; do not sprinkle identical raw values across components.

## Order of precedence

1. **Existing project tokens win by default.** Read CSS variables, Tailwind theme/config, component-library tokens, native platform metrics and Owner decisions. In `design` and `improve`, reuse the existing scale—even if it differs from this table. In an **explicitly authorized `creative` redesign**, study existing tokens as the baseline; intentionally changed *visual* roles may receive new token values selected from the categories below. Never silently install a competing second scale or override non-negotiable brand/platform constraints.
2. **No matching token? Choose from these tables.** Introduce a semantic token and map it to one of the values *in the correct category*. Do not create `17px` or `27px` because it looks nicer than the neighboring steps.
3. **Only if neither works:** a documented accessibility/platform requirement, external asset specification, content-driven geometry or genuine layout constraint may need a nonpreset value. Name the exception and reason; add it to the project's tokens deliberately, rather than as a one-off literal.
4. Use fluid or intrinsic layout (`auto`, `min-content`, `max-content`, `1fr`, `minmax()`, `100%`, and token-based `clamp()/calc()`) rather than fixing widths/heights just to satisfy a table. Calculated sizes derived from a token/constraint are not new arbitrary design presets.

**Do not** round genuine data, exact icon/asset dimensions, device viewports, chart geometry, or required accessibility hit areas to the nearest step. The table is for **design choices**, not for falsifying measurements or breaking platform behavior.

## A. Spacing and fixed spatial sizes

**Source-inspired 16 px-based scale.** Values in pixels; use equivalent `rem` tokens where appropriate. A spacing choice must come from this row, not arbitrary multiples of four.

| Purpose | Allowed values (px) | Typical decisions |
| --- | --- | --- |
| Zero space | **0** | Remove an unnecessary gap or inset |
| Micro / intra-control | **4 · 8 · 12 · 16** | Icon-label gap, inline gap, control padding, compact list spacing |
| Component / section | **24 · 32 · 48 · 64** | Panel padding, grouped controls, section separation |
| Page / large sections | **96 · 128 · 192 · 256** | Large composition intervals, dedicated empty/hero regions when justified |
| Large fixed geometry | **384 · 512 · 640 · 768** | Optional fixed max-size/large module, **not** a default sidebar width |

**Relations beat individual numbers:** gaps between unrelated groups should generally exceed gaps inside a group. Don't add huge whitespace to a dense operations console because a scale includes `256px`.

## B. Typography

| Category | Allowed values | Use |
| --- | --- | --- |
| Font size (px) | **12 · 14 · 16 · 18 · 20 · 24 · 30 · 36 · 48 · 60 · 72** | Choose only roles the product actually needs. `12px` is not a universal legal minimum for every user/context |
| Weight | **400 · 500 · 600 · 700** | Normal → medium → emphasis → strong; respect platform/variable-font constraints |
| Unitless line-height | **1 · 1.125 · 1.25 · 1.5 · 1.75** | Tighter for display, more open for body; preserve localization and readability |
| Reading measure (`ch`) | **45 · 60 · 75** | Optional max-width for long-form prose; irrelevant to tables or code editors |

Use an actual text hierarchy: size, weight, color, and whitespace together. Respect system text scaling and accessible zoom. A fixed font-size table cannot override reader settings.

## C. Corners, borders, icons and controls

These are **Always UI-curated categories**, not a claim that Refactoring UI mandates these exact values.

| Category | Allowed values | When to choose |
| --- | --- | --- |
| Border width (px) | **0 · 1 · 2** | 0 for no border; 1 for subtle boundaries; 2 for strong functional emphasis, if contrast passes |
| Corner radius (px) | **0 · 4 · 6 · 8 · 12 · 16 · 24** | Match the project's geometry; use one or two main radius roles |
| Fully pill-shaped radius | **9999px** or **50%** | Only when the design intentionally calls for pills or circles |
| Icon size (px) | **12 · 16 · 20 · 24 · 32** | Pick from available icon glyph sizes; preserve aspect ratio and optical alignment |
| Control visual height (px) | **28 · 32 · 36 · 40 · 44 · 48 · 56** | 28–36 for dense desktop when usable; 44–56 commonly easier to target on touch |
| Z-layer index | **0 · 10 · 20 · 30 · 40 · 50** | Local overlay/stack ordering; use semantic layer tokens, not scattered `z-index: 9999` |

**Accessibility exception:** minimum target dimensions, focus ring geometry and platform-native controls are determined by applicable standards/host behavior, even if they fall between these presets. A `32px` visual button may have a larger transparent hit target.

## D. Opacity and color stops

| Category | Allowed values | Notes |
| --- | --- | --- |
| Opacity / alpha | **0 · 0.05 · 0.1 · 0.2 · 0.4 · 0.6 · 0.8 · 1** | Background tints, overlays and intentional transparency; do not use low opacity to hide required text |
| Palette stop identifiers | **50 · 100 · 200 · 300 · 400 · 500 · 600 · 700 · 800 · 900** | Optional shade scale *names*, not literal brightness or prescribed colors |
| Number of named color roles | **not prescribed** | Product identity, semantics and content determine which color families are necessary |

The table does **not** provide a universal brand color or fixed HEX/HSL values. Reuse existing palette tokens; if the product needs a new brand or semantic ramp, design and approve actual colors with contrast checks instead of pretending a generated blue/purple is a neutral standard. Preserve embedded user artwork and meaningful data visualization colors.

## E. Elevation and shadows

Use **none by default** when surfaces/spacing already provide hierarchy. If elevation is meaningful, choose an **entire preset**, not custom x/y/blur/alpha fragments. These are Always UI's adapted recipes; the named five-step idea follows the reference source.

| Token | Fixed shadow declaration | Appropriate use |
| --- | --- | --- |
| `none` | `none` | Default flat surfaces, lists, panels without elevation |
| `elevation-1` | `0 1px 3px rgb(0 0 0 / 0.10)` | Subtle raised control |
| `elevation-2` | `0 4px 6px rgb(0 0 0 / 0.10)` | Small floating menu |
| `elevation-3` | `0 5px 15px rgb(0 0 0 / 0.10)` | Raised popover |
| `elevation-4` | `0 10px 24px rgb(0 0 0 / 0.20)` | Floating panel |
| `elevation-5` | `0 15px 35px rgb(0 0 0 / 0.20)` | High-elevation dialog when appropriate |

Shadows are **not required** on every card or button. In dark themes, elevation may instead be conveyed by semantic surface colors. High-contrast/focus indications must not depend on a subtle shadow.

## F. Motion

These are **curated default durations**, not a claim that Refactoring UI supplied these numbers.

| Purpose | Allowed duration (ms) | Notes |
| --- | --- | --- |
| Instant / reduced motion | **0** | Keep state changes immediate when requested |
| Short feedback | **100 · 150** | Press, highlight, small local transitions |
| Standard UI transition | **200 · 300** | Drawer, popover, disclosure, panel switching where motion helps |
| Deliberate transition | **400** | Only if it supports comprehension; never a universal default |

Choose easing from the product's existing motion tokens or platform primitives. Do not invent a unique duration for every component. Match behavior to motion-reduction preferences and allow fast direction reversal.

## G. Responsive breakpoints

| Category | Allowed initial thresholds (px) | Guidance |
| --- | --- | --- |
| Responsive design breakpoints | **480 · 640 · 768 · 1024 · 1280 · 1536** | Use existing framework breakpoints first; choose **only thresholds where the interface actually needs to reorganize** |

Breakpoints are **candidates**, not device labels or a demand to create six `@media` queries. Content-driven fit, platform rules and accessibility can justify a different threshold as an explicitly documented project token.

## Choosing and checking values

**Design-mode procedure**

1. Inspect existing tokens. In `design` and `improve`, reuse them for their respective categories. In an authorized `creative` redesign, identify which stylistic roles may deliberately change while retaining coherent tokens, using the tables for any new fixed choices.
2. If not, select the few scales needed from this document and create named tokens in the project's style language.
3. Use those tokens consistently in spacing, typography, controls, decoration and interaction. Avoid one-off numbers.
4. In the design handoff, identify the selected scales briefly (for example: `space 4/8/16/24; type 14/16/24; radius 8; control 40; elevation none`).
5. Before completion, scan for unexplained raw literals (including Tailwind arbitrary-value classes such as `p-[17px]`). Replace with existing/project preset tokens, or explicitly document the justified exception.

**Check-mode procedure**

Report **numeric drift** only when ad-hoc values introduce inconsistency *relative to that project's own system*. Do not order an established product to adopt these fallback values. No automatic migration or broad normalization during a local UI review.

## Sources and distinction

- **Reference:** [s0xDk/refactoring-ui-skill](https://github.com/s0xDk/refactoring-ui-skill/blob/main/SKILL.md) — spacing/sizing, type/weight, fixed opacity, palette steps and staged elevation idea. These numeric patterns are cited for provenance, not included as a dependency.
- **Always UI additions:** radius, border, icon, control-height, z-layer, motion and breakpoint tables; elevation recipes; precedence/exception handling; table-based workflow.
- A separate [Refactoring UI Skill implementation](https://github.com/edisonmbli/refactoring-ui-skill/blob/main/skills/refactoring-ui/SKILL.md) also emphasizes detecting existing tokens **before** using reference defaults. This document adopts that integration principle, not its full design opinions.

Always UI does **not** invoke, install or depend on a Refactoring UI Skill at runtime. The preset table is an owned reference inside this repository.
