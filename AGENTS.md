# Codex instructions — Zibblu

This file defines project expectations. Read the linked docs before changing a module. The requested `stop-that-shit` behavior is the external **stop-that-shit** Skill / Guard: instructions here cannot install or arm it by themselves.

## Scope contract

1. **One module at a time:** 01 Chat & Files; 02 LAN Private Chat; 03 AI Assistants. Only touch code and docs required for the current task.
2. **Don't overdesign:** no speculative abstractions, generic plugin architecture, unused services, compatibility shims, broad refactors or decorative tests.
3. **No dependency churn** unless the requested feature truly needs it and the task author approves. Follow existing stack and patterns.
4. **Security boundaries are requirements, not optional polish:** private mode must fail closed; the AI must never get write-capable tools or unconstrained chat/file access.
5. **Finish the complete assigned module before testing.** Then run the minimum focused verification: the primary user flow and critical failure/permission boundary. No repeated full-repo test/build/audit loops without evidence of a relevant failure.
6. Do not imply that something is secure, ephemeral, encrypted or tested unless the implementation and observed evidence support it.
7. No secrets, API keys, personally identifying fixtures or actual chat records in commits, logs or screenshots.

## stop-that-shit in Codex

The first non-empty line of each **actual Codex task prompt** should contain exactly one directive, not a quoted code example:

- Implementation: `$stop-that-shit change -- Implement the assigned Zibblu module within its documented acceptance criteria. Avoid unrelated changes and run minimal focused verification only after completion.`
- Review: `$stop-that-shit review -- Review this PR against the assigned module and security constraints. Report actionable defects; make no edits.`

For stronger enforcement, install the Codex Skill **and Guard** using the upstream documentation: https://github.com/lennney/stop-that-shit . A Skill-only instruction is advisory. An installed Guard needs an explicit mode to arm; don't claim it blocked an action without evidence.

Use `lock ... files=...` only when the complete file boundary is already known. Don't invent file lists. Don't grant `deps=allow`, `hash=allow` or agent budgets unless the real task calls for them.

## Module rules

- **01 Chat & Files:** normal chats persist; file transfer is resumable and integrity-checked; clear error states.
- **02 Private Chat:** both users opt in; same reachable LAN + authenticated P2P only; no relay fallback; no private payload in persistent DB, analytics, notifications or cloud AI. End/timeout/crash cleanup is best-effort; never promise screenshot/OS-swap erasure.
- **03 AI Assistants:** `@custom-name` is a one-shot request; user chooses an exact message scope and optional file list; AI can read only this snapshot. No write APIs, code execution, filesystem mutation, background monitoring or implicit private-mode cloud access.

## How to deliver a module

- Make the smallest coherent change that satisfies the module's acceptance criteria in [docs/ROADMAP.md](docs/ROADMAP.md).
- Update the affected docs only if observable behavior or a decision changed.
- Run minimal focused tests *once at module completion*; correct failures and re-run only those relevant checks.
- In the PR summary give: implemented behavior, changed areas, test command/result, limitations and any open security concern.
- Stop after the requested work. Do not begin the next module, merge, deploy or publish releases without instruction.

If requirements conflict, preserve privacy/permission safety and ask for a specific decision instead of silently weakening it.
