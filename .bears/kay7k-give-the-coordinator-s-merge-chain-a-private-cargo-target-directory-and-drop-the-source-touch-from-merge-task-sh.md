---
id: kay7k
title: Give the coordinator's merge chain a private cargo target directory and drop the source touch from merge-task.sh
status: open
priority: P1
created: "2026-09-17T20:27:30.931725616Z"
updated: "2026-09-17T20:27:30.931725616Z"
tags:
  - infra
  - docs
parent: yq6c3
---

## Summary
`merge-task.sh` touches every file under `src`, `tests` and `migrations` before the backend chain because the coordinator's `main` shares `orchestrator/target` with the subagent worktrees and has been handed a sibling's stale test binary. The touch forces a 65 s rebuild and a 29 s clippy run on every merge even when one file changed. Give the coordinator's chain its own target directory (`orchestrator/target-main`) that no worktree ever builds into; then nothing in it can be stale, the touch goes, and the chain runs incrementally. Subagents keep sharing `orchestrator/target` as today.

## Documents
- `.claude/skills/implement-epic/SKILL.md` "Merging a branch" and "Pitfalls seen in practice" → "Shared cargo target directory".
- `.claude/skills/implement-epic/references/dispatch-prompt.md` (the build-speed bullet says the coordinator re-runs the chain "in a clean rebuild on merge").
- `README.md` "Development" if it names the target directory; `CLAUDE.md` "Code quality" is unchanged (the commands are the same).

## Acceptance criteria
- [ ] `merge-task.sh` exports `CARGO_TARGET_DIR="$REPO/orchestrator/target-main"` for the backend chain and no longer runs `find src tests migrations -type f -exec touch {} +`; when the merged commits touch `orchestrator/migrations/`, it still touches `tests/common/db.rs`, `tests/migrations.rs` and `src/main.rs` so the embedded `Migrator` recompiles.
- [ ] `.gitignore` already ignores `target/`; add `target-main/` (or change the pattern to `target*/`).
- [ ] The script prints which target directory it is using and asserts that the chain output contains `Compiling mars-orchestrator` on the first run after a source change (the existing check stays; it now passes for the right reason).
- [ ] "Closing the epic" step 1 (the complete chains on `main` once more) uses the same private directory.
- [ ] The skill's pitfall paragraph is rewritten: the shared directory is a subagent-side hazard only; the coordinator is immune by construction; the `touch` advice for subagents stays.
- [ ] The dispatch template's bullet says the coordinator re-runs the chain in its own target directory, not "a clean rebuild".
- [ ] Measured: a merge of a one-file change in `src/` runs the backend chain (both clippy runs and the full suite compile) in incremental time; state the number in the commit message. The first run after this change builds the whole dependency graph once (budget about the size of `orchestrator/target`, 18 GB today; say so in the skill's environment check).

## Implementation notes
- Files: `.claude/skills/implement-epic/scripts/merge-task.sh`, `.claude/skills/implement-epic/SKILL.md`, `.claude/skills/implement-epic/references/dispatch-prompt.md`, `.gitignore`, `README.md`.
- The frontend chain is unaffected.
- Keep `cargo fmt --check` first; it is independent of the target directory.
- Commit as `docs(infra)` per the skill's "Suggesting improvements" section, with the measurement in the body.

## Edge cases
- Disk: two full target directories. If `df` shows less than 25 GB free, the script should say so rather than fail mid-link; a one-line check before the chain is enough.
- The Engine CI workflow and the local `tests/engine.rs` are unaffected; they use whatever target directory the caller sets.

## Testing
- Run `merge-task.sh` on a throwaway branch with a one-line change and confirm the timing; run the full chain from "Closing the epic" once in the new directory.
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes with `CARGO_TARGET_DIR=orchestrator/target-main`.

## Documentation
- `SKILL.md` and `dispatch-prompt.md` as above; `README.md` "Development" only if it mentions `CARGO_TARGET_DIR`.