# Visual discipline — four shared rules

Use this reference for all four modes: `creative`, `design`, `improve` and `check`. These are **decision tests**, not a required aesthetic. The goal is to reduce visual competition without turning expressive brands into grayscale templates.

**First separate the product's own UI chrome from its content.** A design-review app may display a client's neon poster, colorful dashboard, or gradient artwork. Judge the app's navigation, controls, overlays, labels, and status feedback independently of the artwork being previewed. Do not recolor user-generated content to make the shell look quieter.

## V01 — Color restraint

**Design**
- Assign colors an intentional job before distributing them: brand/action, neutral surfaces/text, selection/focus, and semantic states such as error, warning, success, and pending.
- Use an established brand palette and semantic tokens; don't add a new vivid accent for each widget. Keep supporting actions and passive metadata calmer than the primary task.
- Multiple saturated colors are fine if required for visualization categories, content, an established expressive brand, or distinct semantics. Ensure they do not compete for the same meaning.

**Check**
- Can users tell what the most important action or status is when several colored elements appear together?
- Does one color mean several unrelated things (e.g. active navigation, error, decoration), or do several bright colors mean the same thing?
- Are decorative accents competing with alerts, focus, selected objects, or the main CTA?
- Would a neutral treatment for one *nonessential* element clarify hierarchy without losing meaning?

**Do not:** impose a fixed color count, forbid neon, desaturate meaningful charts/artwork, or replace color-independent state cues with color alone.

## V02 — Button hierarchy

**Design**
- Within each local action group, identify the primary next step. Choose a clear primary treatment for it; secondary, tertiary, and utility actions should usually have less visual weight.
- Use filled color, outline, text, icon-only, and destructive treatments according to function, urgency, and platform conventions—not as a grab bag.
- Add a button icon only when it improves recognition, disambiguates actions, or is an established platform pattern. Don't turn every label into an icon + pill.
- Multiple equally prominent actions can be appropriate if the choices are genuinely peers. Different zones can each have their own local primary action.

**Check**
- Are two or more bright buttons competing *within the same decision area*? Is the actual next step clear?
- Do large pills, saturated fills, icon backgrounds, or glowing outlines make routine tools look like CTAs?
- Are icons repeating words, or are outline/shape treatments conveying no additional state?
- Can a secondary control be made quieter while maintaining affordance and accessibility?

**Do not:** ban rounded or pill buttons, remove recognizable icons just to be minimalist, or hide destructive/critical actions where they need clear signposting.

## V03 — Decoration justification

**Design**
- For each prominent gradient, glow, shadow, glass layer, badge, pill background, border, or animation, be able to name its purpose: hierarchy, interaction state, grouping, depth, legibility, or deliberate brand expression.
- If two treatments communicate the same thing, choose the simpler effective combination. Prefer the project's tokens and existing visual conventions over one-off effects.
- Apply effects where they strengthen the intended focal point; avoid making every card and toolbar feel like a featured element.

**Check**
- What would the user lose if this effect disappeared? If the answer is "nothing," consider removing or reducing it.
- Does a gradient/glow make an inactive control look active? Does a shadow imply elevation that the surface doesn't have?
- Are unrelated effects stacked on one component without a clear hierarchy?
- Does the effect hurt contrast, legibility, performance, or reduced-motion behavior?

**Do not:** treat all gradients, shadows, glass or motion as flaws. An expressive brand, a designed artifact in a preview, or a meaningful elevation cue may justify them.

## V04 — Component economy

**Design**
- Before wrapping an item in a card, ask whether it has its own interaction/selection, grouping responsibility, or independent content lifecycle.
- For repeated homogeneous content, consider a list/table, spacing, subtle row hover/selection, or background rhythm before independent boxed cards.
- Use cards when they help compare self-contained objects, preserve distinct boundaries, or support separate actions. Respect existing design conventions.

**Check**
- Does every file, comment, metric and label have its own bordered/rounded container for no functional reason?
- Are cards nested mainly to create "premium" depth rather than identify separate levels?
- Could spacing or one shared panel border communicate exactly the same structure more cleanly?
- Would removing the container harm scanning, selected state, drag targets, or grouping? If so, keep it.

**Do not:** ban cards or borders globally, erase focus indicators, remove touch targets, or collapse semantically distinct content into an unreadable list.

## How to apply these rules

### In `creative`
Apply these rules to the **new** visual language without forcing a muted or generic theme. Creativity may justify stronger colors, geometry or motion when they serve the brand and do not impair the task. Compare the new shell with the inspected baseline; do not treat swapping hue as meaningful redesign.

### In `design`
After choosing an archetype and planning interactions, perform a short **visual discipline pass**: define color roles; decide the primary action in each action group; justify conspicuous effects; choose where cards are actually needed. This is a private design decision process, **not** a request to write a long aesthetic manifesto. Mention only decisions materially relevant to the result.

### In `improve`
Apply the same criteria using the **existing project's** tokens and identity. Fix a small number of observable issues; do not replace navigation, theme or component vocabulary without explicit permission.

### In `check`
Inspect the actual screen and identify **observable** instances before raising an issue. Report location, evidence, user impact, minimal adjustment, and whether the behavior was tested. Mark purely stylistic recommendations as optional; avoid false positives for intentional expressive brands.

### Example: creative delivery workspace

- **Color:** a vivid client mockup displayed on the canvas is **content**, not evidence of an oversaturated app shell. Evaluate shell accents and status colors separately.
- **Buttons:** "Deliver to client" may be the primary action in the header; "Upload revision" might be the local primary action inside a different workflow. Do not count global buttons without considering action zones.
- **Decoration:** a gradient inside the client's concept art is untouched; a decorative glow around every toolbar icon needs a reason.
- **Components:** a selected file card may need a boundary for drag/preview actions, while a long comments list may be clearer with simple rows and a single panel boundary.
