# Repository Skills

These are **vendored repository-scoped Agent Skills** for Codex cloud/CLI/IDE. They are checked into `.agents/skills/`, so a Codex environment that checks out this repository can discover them. Installation of the repository files **does not mean their optional plugin hooks or guards are installed or active**. No hook is configured here, and the included diagnostic script is not auto-executed.

## Installed Skills

| Source project | Repository skill | Use | Provenance |
| --- | --- | --- | --- |
| [lennney/stop-that-shit](https://github.com/lennney/stop-that-shit) | `stop-that-shit` | Bounded changes/reviews and avoiding speculative work | Tag `0.2.5` |
| [DietrichGebert/ponytail](https://github.com/DietrichGebert/ponytail) | `ponytail` | Smallest complete engineering change | `main` snapshot, 2026-11-11 |
| [shadcn/improve](https://github.com/shadcn/improve) | `improve` | **On request only:** read-only codebase audit and implementation plans | `main` snapshot, 2026-11-11 |
| [mattpocock/skills](https://github.com/mattpocock/skills) | `setup-matt-pocock-skills` | Optional one-time engineering workflow setup | `main` snapshot, 2026-11-11 |
| [mattpocock/skills](https://github.com/mattpocock/skills) | `codebase-design` | Module and interface decisions when specifically needed | `main` snapshot, 2026-11-11 |
| [mattpocock/skills](https://github.com/mattpocock/skills) | `diagnosing-bugs` | Reproduce and diagnose difficult bugs | `main` snapshot, 2026-11-11 |
| [mattpocock/skills](https://github.com/mattpocock/skills) | `code-review` | Explicit PR/diff review, not automatic review agents | `main` snapshot, 2026-11-11 |

**Matt Pocock is a collection, not a single skill named `matt-pocock`.** The four listed skills are a deliberate subset, not the whole collection. No other Matt Pocock skill was installed.

Copied skill files were kept unchanged, including their local reference material and the optional diagnostic shell-script template. Snapshot versions are not automatically updated.

## Authority and usage for Zibblu

1. **User request + `AGENTS.md` + `docs/SECURITY.md` + module acceptance criteria** take priority when any bundled skill's generic workflow would add work or weaken a permission boundary.
2. For normal M1 implementation, invoke `$stop-that-shit change -- ...`. Ponytail may help minimize implementation. Finish the complete assigned module; only then run the minimal module acceptance checks described in `docs/ROADMAP.md`. Correct and recheck actual failures.
3. **Necessary integrity hashing is not waste:** M1 explicitly requires verifying resumed file transfers. Do not delete essential verification as “overengineering”.
4. Do **not** invoke `improve`, `setup-matt-pocock-skills`, `code-review` or parallel agents automatically on every task. Use their prescribed flows when explicitly requested or the task actually calls for them. In particular, `improve` can create plans, and some upstream variants can dispatch agents; only do so with the requesting user's authorization.
5. Never execute a copied third-party script just because it exists. Inspect its behavior and confirm it is relevant before use.
6. Codex may load a SKILL.md, but this does **not** install the source repository's Codex plugin or activate **stop-that-shit Guard** / **ponytail hooks**. Install and review those in a supported Codex host separately if runtime blocking is desired.

## Third-party licenses

The upstream projects publish MIT licensing. Their original license notices are retained verbatim:

- [Stop That Shit](licenses/stop-that-shit-MIT.txt)
- [Ponytail](licenses/ponytail-MIT.txt)
- [Improve](licenses/improve-MIT.txt)
- [Matt Pocock Skills](licenses/mattpocock-MIT.txt)

## Validation and maintenance

When adding or updating a skill, ensure its `SKILL.md` frontmatter includes `name` and `description`, and its local references exist. Pin or record source versions, review any script changes and keep license notices. A static file check is **not** proof of automatic invocation or runtime hook enforcement.

Keep future changes scoped. Install additional Matt Pocock skills only when an actual Zibblu workflow requires them.
