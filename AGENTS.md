# Agent Instructions

## Canvas build and architecture

Rust is the runtime and iced is the required GUI toolkit. Build with
`cargo build --workspace`; test with `cargo test --workspace`; lint with
`cargo clippy --workspace --all-targets -- -D warnings`; format with `cargo fmt --all`.
Run the Mac shell with `cargo run -p canvas-mac` and inspect the agent-facing policy
interface with `cargo run -p canvas-core --bin canvas-core-ctl -- --help`.

`config/projects.json` supplies initial project/resource identities. Coherence's
upstream native token crate is vendored with checksums under `vendor/`.
The earlier Python/TOML compiler is exploratory history, not an adopted runtime
or a prerequisite. Do not extend it. Do not duplicate Coherence palettes.
Agora One is the Ubuntu GPU desktop. The interactive project surface is an
isolated headless Sway desktop with wayvnc over an SSH tunnel, independently of
GNOME's lock state. The earlier nested Hyprland session remains exploratory.
OpenCode owns persistent coding conversations; Canvas uses its supported API and
attaches a visible terminal to the same session. Backend control gates serialize
agent inputs and revoke them before aborting a session on human takeover.
Mac AX operations belong in the independently installed Dock Control helper;
normal GUI rebuilds must not overwrite that permission-bearing app.
Window, workspace, and wallpaper behavior implement the narrow traits in
`canvas-core`; macOS and Windows use independently installed permission-bearing
helpers, Sway implements them directly, and future compositors remain adapters.
Old Sway/dotfiles are inspiration, not a restoration target.
Preserve the separation of presentation, orchestration,
transport, compute, and agency in `docs/computing-mesh-design.md`.
Beads epic `dock-96j` tracks v0; `dock-2kf` tracks later mesh phases.

## Naming

The project is Canvas; crates, binaries and modules use that name. Several
identifiers deliberately still say `dock`, because each one names state already
installed on a machine rather than something in this tree: the repository
directory and its GitHub remote, the `fpl-local-dock-canvas` and
`fpl-local-dock-workspaces` MCP registrations, `dev.fpl.dock-canvas` and the
paths in `scripts/setup-mac-canvas-mcp.py`, `.dock/state.json` and `DOCK_STATE`,
the `Dock Control.app` bundle and its `com.futurepresentlabs.dock.control`
Accessibility grant, `ThemeFormat::Dock` and the `Library/Application
Support/Dock` socket paths, and the `dock-*` Bead IDs. Renaming one of those from
inside a refactor orphans the installed copy instead of upgrading it; they move
in a deliberate migration, when someone is present to re-grant or re-register
what breaks. `Dock` and `Studio` are also the names of two project workspaces in
`config/projects.json` — those are project identity, not the old product name.

This project uses **bd** (beads) for issue tracking. Run `bd prime` for full workflow context.

> **Architecture in one line:** Issues live in a local Dolt database
> (`.beads/dolt/`); cross-machine sync uses `bd dolt push/pull` (a
> git-compatible protocol), stored under `refs/dolt/data` on your git
> remote — separate from `refs/heads/*` where your code lives.
> `.beads/issues.jsonl` is a passive export, not the wire protocol.
>
> See [SYNC_CONCEPTS.md](https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md)
> for the one-screen overview and anti-patterns (don't treat JSONL as the
> source of truth; don't `bd import` during normal operation; don't
> reach for third-party Dolt hosting before trying the default).

## Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work atomically
bd close <id>         # Complete work
bd dolt push          # Push beads data to remote
```

## Non-Interactive Shell Commands

**ALWAYS use non-interactive flags** with file operations to avoid hanging on confirmation prompts.

Shell commands like `cp`, `mv`, and `rm` may be aliased to include `-i` (interactive) mode on some systems, causing the agent to hang indefinitely waiting for y/n input.

**Use these forms instead:**
```bash
# Force overwrite without prompting
cp -f source dest           # NOT: cp source dest
mv -f source dest           # NOT: mv source dest
rm -f file                  # NOT: rm file

# For recursive operations
rm -rf directory            # NOT: rm -r directory
cp -rf source dest          # NOT: cp -r source dest
```

**Other commands that may prompt:**
- `scp` - use `-o BatchMode=yes` for non-interactive
- `ssh` - use `-o BatchMode=yes` to fail instead of prompting
- `apt-get` - use `-y` flag
- `brew` - use `HOMEBREW_NO_AUTO_UPDATE=1` env var

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:6cd5cc61 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.

## Agent Context Profiles

The managed Beads block is task-tracking guidance, not permission to override repository, user, or orchestrator instructions.

- **Conservative (default)**: Use `bd` for task tracking. Do not run git commits, git pushes, or Dolt remote sync unless explicitly asked. At handoff, report changed files, validation, and suggested next commands.
- **Minimal**: Keep tool instruction files as pointers to `bd prime`; use the same conservative git policy unless active instructions say otherwise.
- **Team-maintainer**: Only when the repository explicitly opts in, agents may close beads, run quality gates, commit, and push as part of session close. A current "do not commit" or "do not push" instruction still wins.

## Session Completion

This protocol applies when ending a Beads implementation workflow. It is subordinate to explicit user, repository, and orchestrator instructions.

1. **File issues for remaining work** - Create beads for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Handle git/sync by active profile**:
   ```bash
   # Conservative/minimal/default: report status and proposed commands; wait for approval.
   git status

   # Team-maintainer opt-in only, unless current instructions forbid it:
   git pull --rebase
   git push
   git status
   ```
5. **Hand off** - Summarize changes, validation, issue status, and any blocked sync/commit/push step

**Critical rules:**
- Explicit user or orchestrator instructions override this Beads block.
- Do not commit or push without clear authority from the active profile or the current user request.
- If a required sync or push is blocked, stop and report the exact command and error.
<!-- END BEADS INTEGRATION -->

<!-- BEGIN BEADS CODEX SETUP: generated by bd setup codex -->
## Beads Issue Tracker

Use Beads (`bd`) for durable task tracking in repositories that include it. Use the `beads` skill at `.agents/skills/beads/SKILL.md` (project install) or `~/.agents/skills/beads/SKILL.md` (global install) for Beads workflow guidance, then use the `bd` CLI for issue operations.

### Quick Reference

```bash
bd ready                # Find available work
bd show <id>            # View issue details
bd update <id> --claim  # Claim work
bd close <id>           # Complete work
bd prime                # Refresh Beads context
```

### Rules

- Use `bd` for all task tracking; do not create markdown TODO lists.
- Run `bd prime` when Beads context is missing or stale. Codex 0.129.0+ can load Beads context automatically through native hooks; use `/hooks` to inspect or toggle them.
- Keep persistent project memory in Beads via `bd remember`; do not create ad hoc memory files.

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.
<!-- END BEADS CODEX SETUP -->

<!-- BEGIN FPL DESIGN TENETS (managed by fpl) -->

# FPL Systems Design Tenets

These tenets apply across all FPL / Puget Audio codebases and automation. Every agent session (Claude Code, Codex, or otherwise) should treat these as load-bearing defaults, not suggestions — deviate only with an explicit, stated reason.

---

### 1. DRY (Don't Repeat Yourself)
One source of truth per fact. If the same schema, constant, or logic exists in two places, one of them is already wrong and just hasn't been noticed yet. Prefer a shared crate/module/service over copy-paste, even across repo boundaries in the FPL ecosystem (e.g. call `lob`'s Mouser integration — don't re-implement it in the workflow layer).

### 2. Loose Coupling
Modules and services talk through narrow, explicit interfaces — not shared internal state, not reaching into another project's guts. A project should be replaceable without its neighbors caring, as long as the interface contract holds. This is what keeps the "loose tools" (Quotron2, legion-of-bom, mesh-to-step, Panopticon, etc.) glueable instead of tangled.

### 3. Infer/Calculate Over Static/Configured (Good Dynamism)
Derive values from source-of-truth data at the point of use rather than hand-maintaining config that can drift out of sync. If a number can be computed from something else that already exists, compute it — don't cache a stale copy in a config file or constant.

### 4. Design Good Interfaces
Spend real thought on the boundary before implementing what's behind it. A good interface is boring, hard to misuse, and survives its internals being rewritten. Interfaces, persistence formats, and contracts are the genuine one-way doors — get these right up front even when you're moving fast on everything else.

### 5. Commit Early, Commit Often
Small, frequent, reversible commits. History should read as a trail of intent, not a single monolithic diff. This is what makes rollback, bisect, and review actually usable later. Agents are authorized to create local commits for completed, verified work by default; do not leave useful work uncommitted merely because a session is ending. An explicit instruction not to commit still wins. Pushing, merging, releasing, deploying, remote synchronization, and destructive history edits remain separately authorized operations.

### 6. Test Everything
No behavior ships un-verified. Tests are how a system stays trustworthy as an agent (human or LLM) keeps modifying it — they're the guardrail that lets you move fast without re-deriving correctness by hand every time.

### 7. Deterministic Orchestration, Narrow LLM-Assist
Workflows are discrete, deterministic, and reproducible by default (state machines, scripted flows) — not unstructured agent loops deciding what happens next. LLMs assist at specific, bounded points inside a workflow (drafting, scoring, summarizing), never as the thing deciding control flow. If the orchestrator's behavior can't be predicted from reading the code, it's not a workflow, it's a gamble.

### 8. Runbook Parity
Any automated workflow should still be describable as a manual runbook a human could execute if the automation broke. If you can't say "here's how a person would do this step by step," the workflow is probably hiding a business process you don't actually understand yet.

### 9. Self-Validating Over Human-Gated
Default to systems that catch their own mistakes, not systems that add a human review checkpoint. A review gate is a scaling liability disguised as a safety feature. Build the validation into the loop; don't outsource it to future-you's attention.

### 10. Call, Don't Rebuild
If a capability is already owned by another project in the ecosystem, invoke it as a tool — don't fork the logic into the caller. Ownership of a capability lives in exactly one place; everything else is a client of it.

### 11. Boring Technology, Deliberate Sprawl
New tools/services/dependencies earn their place — prefer the boring, already-adopted option over a novel one unless there's a concrete reason. Every new moving part is something that has to be audited, secured, and kept alive; unexamined sprawl is debt, not progress.

### 12. Fail Loud, Fail Fast
Errors surface immediately and visibly — no silent fallbacks, no swallowed exceptions, no "probably fine." A system that fails quietly just moves the cost of the bug to whoever debugs it in production three weeks later.

### 13. Cadence Beats Polish (Where Cadence Is the Point)
For anything whose value comes from consistency over time — content, reporting, outreach — a steady mediocre output beats a sporadic polished one. Save the polish budget for the genuine one-way doors (tenet #4); don't spend it gold-plating things that just need to keep showing up.

---

*Living document — extend as new patterns prove themselves, don't just accumulate opinions. If a tenet stops being true of how FPL actually builds things, cut it rather than let it rot into aspirational fiction.*

<!-- END FPL DESIGN TENETS -->
