---
name: implement-epic
description: Implement a Bears epic end to end by dispatching one fresh Opus subagent per task in an isolated git worktree, in dependency-ordered waves, while this session reviews every branch, merges it onto main, runs the quality chains, tracks progress in Bears and pushes with CI green. Use this whenever the user wants an epic, a batch of Bears tasks or "the next epic" implemented, wants tasks worked in parallel by subagents, or asks you to review and merge subagent work; also when they say "start implementing", "do the next epic" or "work through the ready tasks", even if they do not mention subagents.
---

# Implement an epic with subagents

You are the coordinator. Subagents write code; you plan waves, review diffs, merge, verify, keep Bears current and push. Never implement a task yourself while running this workflow unless the user asks for it: fresh-context subagents keep each task's context small, and your context is reserved for reviewing all of them.

## Before starting

1. Confirm the environment once and note the results for the report:
   - `podman --version` and the socket at `$XDG_RUNTIME_DIR/podman/podman.sock`; export `DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock` for every cargo command (the testcontainers health test and the engine tests need it). Without an engine, tell subagents to write Docker-dependent tests anyway and skip them locally with `-- --skip <prefix>`; CI runs them.
   - `git status` clean on `main`, `main` equal to `origin/main`. If `.bears/` is dirty, commit it first as a `chore(infra)` commit.
   - Bears tools reach this repository: `mcp__bears__list_epics` shows the Mars epics. The MCP server is bound to the session's starting directory; if it shows another project's tasks, use `bea ... --json` from the repository root instead (CLAUDE.md, rule 2).
2. Pick the epic: `list_epics`, then `get_graph` with `epic: <id>` and `get_task` on every task. Build the waves from the dependency graph: wave 1 is every task without unmet dependencies, wave n+1 is what becomes ready when wave n is merged. Tasks in the same wave run in parallel.
3. Tell the user in one line which epic and how many waves, then start; do not wait for confirmation unless something in the epic is unclear.

## Dispatching a task

For every task in the current wave:

1. `start_task` with assignee `claude-opus-subagent`.
2. Launch an `Agent` with `subagent_type: general-purpose`, `model: opus`, `isolation: worktree`, and a prompt built from `references/dispatch-prompt.md`. Paste the full task body from Bears; subagents do not have Bears access and must not touch `.bears/`.
3. Fill the prompt's concurrency note: name the files other agents in the same wave are editing and ask for minimal, well-separated edits there (for example "another agent adds two lines to `src/prelude/mod.rs`; keep your edit to your own two lines"). This turns most merge conflicts into trivial ones.
4. Launch every agent of a wave in one message so they run concurrently. Then stop and wait; the completion notification carries the report.

Why a fresh Opus agent per task: each task body is written to be self-contained, so a fresh context implements it faithfully and does not carry assumptions from a sibling task. Why worktrees: parallel agents cannot share one working tree, and the branch gives you a diff to review before anything reaches `main`.

Known harness behaviour: the worktree may be created from `origin/main`, not local `main`, and sometimes under a different repository if the session's working directory moved. The prompt template therefore begins with `git reset --hard main` and a remote check; keep those lines.

## Reviewing a branch

When a report arrives, review before merging. Read the diff, not the report:

```bash
git -C /home/lhelge/dev/mars log --oneline main..<branch>
git -C /home/lhelge/dev/mars show <sha> --stat
git -C /home/lhelge/dev/mars show <sha> -- <the files that matter>
```

Check, in this order:

- **Scope**: only the task's files plus the documents it was told to update; no `.bears/` changes; no unrelated refactors; nothing pushed.
- **Acceptance criteria**: walk the task's checklist against the code. The report says what was built; the diff says what is true.
- **Rule 1 (documentation)**: any behaviour, endpoint, schema, config variable or crate change has its document change in the same commit (`SPEC.md`, `ARCHITECTURE.md`, `README.md`, `docs/data-model.md`, `CLAUDE.md`, or a new ADR).
- **Rule 3 (secrets)**: fixtures and `.env.example` values are obviously fake; nothing secret reaches logs, error bodies or `Debug` output.
- **Conventions**: `use crate::prelude::*`, repositories own SQL, routes export `routes()`, DTOs private, structured `tracing` fields, Conventional Commit with the task id and the attribution trailer.
- **Claims versus evidence**: the report lists the commands it ran. Re-run them yourself during the merge (the script does). Treat "could not verify" items as your job: verify them if the environment allows, otherwise carry them into the final report.
- **Deviations**: a subagent that changed a document because a task assumption was wrong (a renamed crate feature, a missing default) did the right thing; check the document edit is factual and minimal. A subagent that silently narrowed scope did not; send it back with `SendMessage` or fix the gap yourself if it is a few lines.

Findings that are real but out of the task's scope become new Bears tasks (`create_task` with `parent` set to the epic and `depends_on` the current task), not silent fixes and not TODOs.

## Merging a branch

Use the bundled script; it does the repetitive part the same way every time:

```bash
.claude/skills/implement-epic/scripts/merge-task.sh <branch> --task <id>
```

It cherry-picks the branch's commits onto `main` (cherry-pick rather than fast-forward, because the branch base is often behind `main`), removes the worktree and branch, forces a rebuild of the orchestrator crate when backend files changed, runs the backend and/or frontend quality chains for the areas the commits touched, and on success marks the task done with `bea`. On a conflict it stops with the conflicted files listed; resolve them, `git cherry-pick --continue`, then rerun the script with `--after-conflict` to finish the cleanup and verification.

After each merge, push `main`:

```bash
git -C /home/lhelge/dev/mars push origin main
```

Pushing per merge keeps `origin/main` equal to `main`, so the next wave's worktrees start from the right base, and CI validates each task on its own rather than a whole epic at once. Do not wait for CI before dispatching the next wave; check runs with `gh run list` when a wave is done and stop dispatching if one is red.

Commit `.bears/` state changes at the end of each wave as `chore(infra): track <epic> progress in Bears (<epic id>)`; the tracker files are part of the repository, and a subagent's branch must never carry them.

## Pitfalls seen in practice

- **Shared cargo target directory.** Pointing subagents at `orchestrator/target` through `CARGO_TARGET_DIR` avoids a cold build per worktree, but cargo can reuse a test binary whose compile-time `CARGO_MANIFEST_DIR` points at a deleted worktree, which fails tests that read files relative to the manifest. Worse, two worktrees of the same package can collide so that a subagent's clippy or test run reuses a sibling's artifact and reports "clean" without compiling the subagent's code at all. The dispatch template therefore asks subagents to `touch` their sources before the final run and to confirm the `Compiling mars-orchestrator (<worktree>)` line; the merge script touches every source file before the chain (`cargo clean -p` alone was seen leaving a stale test binary in place), so the coordinator's verification is always a real build; check that its output contains the `Compiling mars-orchestrator` line and that each test binary reports the expected test count. Keep the shared directory, it saves minutes per task.
- **Embedded migrations.** `sqlx::migrate!` embeds the SQL at compile time and cargo does not always notice a changed `.sql` file. A subagent that adds or edits a migration must `touch` the files that use the `Migrator` (`tests/common/db.rs`, `tests/migrations.rs`, `src/main.rs`) before testing, or a stale binary reports the old migration count.
- **Attribution trailer.** A subagent running as Opus tends to write its own model name into `Co-Authored-By`. The template now says to use the coordinator's line verbatim; if a report still shows a different line, amend the commit message on the branch before merging (`git -C <worktree> log -1 --format=%B | sed ... | git -C <worktree> commit --amend -F -`).
- **Process cleanup.** A `pkill -f <pattern>` also matches the shell running it and any unrelated process with that string in its command line; one such call killed a developer's dev server. Subagents and you: start background servers with their pid recorded, kill by pid, and never `pkill -f` a bare tool name.
- **Port collisions.** Vite silently moves from 5173 to the next free port while Playwright waits on `PLAYWRIGHT_BASE_URL`; set the variable explicitly when 5173 is busy rather than killing whatever holds it.
- **Two tasks editing one file.** Expect conflicts in `src/prelude/mod.rs`, `frontend/package.json`, `Cargo.toml` and `README.md`. The concurrency note in the dispatch prompt keeps them to a few lines; resolve them by hand rather than reordering waves.
- **Bears MCP binding.** The server follows the session's starting directory. If `start_task` says "task not found", you are talking to another repository's tracker; switch to `bea` from the Mars root.

## Closing the epic

When the last task is merged and pushed:

1. Run the complete chains on `main` once more: backend from CLAUDE.md "Code quality" with `DOCKER_HOST` set and no `--skip`; frontend including `npm run test:e2e`.
2. `complete_task` on the epic (or `bea done <epic>`), commit `.bears/`, push, and wait for the CI runs on that commit (`gh run watch <id> --exit-status`).
3. Report to the user: which tasks merged (one line each), what CI says, every deviation from the task texts, every follow-up task filed, anything that could not be verified and why, and the next ready epic.

## Suggesting improvements

This skill records how the scaffolding epic was run. When a wave shows a repeated cost (the same conflict, the same manual verification, the same prompt correction), update the skill or the dispatch template in the same session as a `docs(infra)` commit, so the next epic starts from the improved version.
