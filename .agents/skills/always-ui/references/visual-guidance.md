# Optional visual guidance — minimal / tonal UIs

Read this file only when the user or existing product design calls for a restrained, tonal, low-decoration aesthetic. These are **contextual heuristics**, not global rules or accessibility exemptions.

## Surface hierarchy

- Prefer small lightness shifts in one established color family to separate canvas, sidebar, panels, and inset regions.
- Prefer one meaningful boundary cue to stacked outlines. Removing decoration is worthwhile only when users can still perceive grouping, affordance, and selection.
- A design may use a 2 px separator where its scale, density, and contrast justify it. **Do not globally enforce 2 px borders** or forbid the use of any border inside a component.

## Corner geometry

- When a product supports continuous/smoothed corners, use them consistently with its own design system. Do not simulate unsupported corner curvature at the cost of clipping or performance.
- On nested surfaces, visually align inner and outer corner shapes; a starting geometric heuristic is outer radius ≈ inner radius + visible inset, not a strict pixel equation.

## Focus treatment

- A dual-ring treatment (tight inner indicator plus softer outer glow) can be a useful brand choice. It must remain visibly different from hover/selected/error states and maintain a clearly visible keyboard focus indication.
- Shadows can disappear in Windows forced-colors or other high-contrast modes. Keep a reliable `outline` or platform-equivalent fallback; test keyboard operation. Never make `outline: none` the default without an appropriate replacement.

## Typography

- A small number of purposeful type roles often reads better than many near-identical font sizes. **Four sizes per page is a heuristic, not a cap.** Respect legibility, user zoom, accessibility scaling, and complex data needs.
- In dark themes, optical weight may look stronger. Compare the actual rendered text in light/dark contexts before changing weight. If a variable font supports intermediate weights, fine-tuning can help; **do not blindly multiply weights by 0.9** or assume every platform font supports 360.
- Short, positive labels help toggle comprehension, but prioritize clarity and localization. For Chinese, 4–6 characters may be an editing target, not a reason to obscure meaning.

## Motion

- Lightweight, interruptible drawer movement can feel responsive. Keep user control immediate and honor reduced-motion settings. Do not demand spring effects where the product is intentionally static or platform conventions differ.
