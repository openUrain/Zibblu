# Repository Skills

These are **repository-scoped Agent Skills** for Codex cloud/CLI/IDE, including vendored third-party engineering skills and Owner-authored interaction rules. They are checked into `.agents/skills/`, so a Codex environment that checks out this repository can discover them. Installation of the repository files **does not mean their optional plugin hooks or guards are installed or active**. No hook is configured here, and the included diagnostic script is not auto-executed.

## Installed Skills

| Source project | Repository skill | Use | Provenance |
| --- | --- | --- | --- |
| [lennney/stop-that-shit](https://github.com/lennney/stop-that-shit) | `stop-that-shit` | Bounded changes/reviews and avoiding speculative work | Tag `0.2.5` |
| [DietrichGebert/ponytail](https://github.com/DietrichGebert/ponytail) | `ponytail` | Smallest complete engineering change | `main` snapshot, 2026-11-11 |
| [shadcn/improve](https://github.com/shadcn/improve) | `improve` | **On request only:** read-only codebase audit and implementation plans | `main` snapshot, 2026-11-11 |
| [mattpocock/skills](https://github.com/mattpocock/skills) | `codebase-design` | Module and interface decisions when specifically needed | `main` snapshot, 2026-11-11 |
| [mattpocock/skills](https://github.com/mattpocock/skills) | `diagnosing-bugs` | Reproduce and diagnose difficult bugs | `main` snapshot, 2026-11-11 |
| [mattpocock/skills](https://github.com/mattpocock/skills) | `code-review` | Explicit PR/diff review, not automatic review agents | `main` snapshot, 2026-11-11 |
| Owner-authored | `interaction-design-rules` | UI interaction detail rules and scoped UI review | Owner-uploaded `SKILL.md`, 2026-11-18 |

**Matt Pocock is a collection, not a single skill named `matt-pocock`.** The three listed skills are a deliberate subset, not the whole collection. No other Matt Pocock skill was installed.

Vendored third-party skill files were kept unchanged, including their local reference material and the optional diagnostic shell-script template. The Owner-authored `interaction-design-rules/SKILL.md` is added verbatim; it is not a third-party dependency. Snapshot versions are not automatically updated.

## Authority and usage for Zibblu

1. **User request + `AGENTS.md` + `docs/SECURITY.md` + module acceptance criteria** take priority when any bundled skill's generic workflow would add work or weaken a permission boundary.
2. For normal M1 implementation, invoke `$stop-that-shit change -- ...`. Ponytail may help minimize implementation. Finish the complete assigned module; only then run the minimal module acceptance checks described in `docs/ROADMAP.md`. Correct and recheck actual failures.
3. **Necessary integrity hashing is not waste:** M1 explicitly requires verifying resumed file transfers. Do not delete essential verification as “overengineering”.
4. Do **not** invoke `improve`, `code-review` or parallel agents automatically on every task. Use their prescribed flows when explicitly requested or the task actually calls for them. In particular, `improve` can create plans, and some upstream variants can dispatch agents; only do so with the requesting user's authorization.
5. Never execute a copied third-party script just because it exists. Inspect its behavior and confirm it is relevant before use.
6. Codex may load a SKILL.md, but this does **not** install the source repository's Codex plugin or activate **stop-that-shit Guard** / **ponytail hooks**. Install and review those in a supported Codex host separately if runtime blocking is desired.

## UI design skill policy

- This repository does **not** vendor `impeccable` or `refactoring-ui` (RUI). **Do not invoke either one for Zibblu UI planning, implementation, or review**, even if available from the Agent's global/user-level skills. Do not install or add them back as part of UI work.
- For UI design, implementation, and review, read `docs/DESIGN_SYSTEM.md` and use the Owner's `$interaction-design-rules` for applicable page/platform details. The design system remains authoritative for Zibblu identity, platform navigation, and product-specific interaction decisions; the Skill adds concrete interaction/visual constraints without replacing the design system.
- Apply desktop-specific instructions only to desktop UI. When constraints conflict, do not silently redesign: prioritize security/privacy, accessibility, platform operability, and Owner-approved product decisions, and report the conflict. Keep changes inside the current Issue's authorized scope.
- Engineering skills may assist with code correctness, debugging, review, and minimal changes; they do not choose UI aesthetics. Report conflicts between design constraints instead of silently normalizing the interface toward generic patterns.

## Third-party licenses

The upstream projects publish MIT licensing. Their original license notices are retained verbatim:

- [Stop That Shit](licenses/stop-that-shit-MIT.txt)
- [Ponytail](licenses/ponytail-MIT.txt)
- [Improve](licenses/improve-MIT.txt)
- [Matt Pocock Skills](licenses/mattpocock-MIT.txt)

## Validation and maintenance

When adding or updating a skill, ensure its `SKILL.md` frontmatter includes `name` and `description`, and its local references exist. Pin or record source versions, review any script changes and keep license notices. A static file check is **not** proof of automatic invocation or runtime hook enforcement.

Keep future changes scoped. Install additional Matt Pocock skills only when an actual Zibblu workflow requires them.
