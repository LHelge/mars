---
name: implement-epic
description: Implement a Bears epic end to end by dispatching one fresh Opus subagent per task in an isolated git worktree, in dependency-ordered waves, while this session reviews every branch, merges it onto main, runs the local quality chains as the gate for each task, tracks progress in Bears and pushes once at the end of the epic with CI green. Use this whenever the user wants an epic, a batch of Bears tasks or "the next epic" implemented, wants tasks worked in parallel by subagents, or asks you to review and merge subagent work; also when they say "start implementing", "do the next epic" or "work through the ready tasks", even if they do not mention subagents.
---

# Implement an epic with subagents

You are the coordinator. Subagents write code; you plan waves, review diffs, merge, verify locally, keep Bears current and push once when the epic closes. Never implement a task yourself while running this workflow unless the user asks for it: fresh-context subagents keep each task's context small, and your context is reserved for reviewing all of them.

## Before starting

1. Confirm the environment once and note the results for the report:
   - `podman --version` and the socket at `$XDG_RUNTIME_DIR/podman/podman.sock`; export `DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock` for every cargo command (the testcontainers health test and the engine tests need it). Without an engine, tell subagents to write Docker-dependent tests anyway and skip them locally with `-- --skip <prefix>`; CI runs them.
   - Disk: `df -h .` shows at least 25 GB free. The build tree is two full target directories, `orchestrator/target` for the subagents and `orchestrator/target-main` for your own chain, each about the size of the other (18 GB today). On a fresh checkout the first merge pays one cold build of the whole dependency graph in `target-main` (about two minutes of compiling on sixteen cores, 17 GB written); every merge after it is incremental. Both are gitignored and either can be deleted to reclaim the space.
   - `git status` clean on `main`. `main` is equal to `origin/main`, or ahead of it only by merged task commits and Bears commits from an epic that was interrupted before its closing push; anything else is resolved before dispatching. If `.bears/` is dirty, commit it first as a `chore(infra)` commit.
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
git -C ~/dev/mars log --oneline main..<branch>
git -C ~/dev/mars show <sha> --stat
git -C ~/dev/mars show <sha> -- <the files that matter>
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

It cherry-picks the branch's commits onto `main` (cherry-pick rather than fast-forward, because the branch base is often behind `main`), removes the worktree and branch, runs the backend and/or frontend quality chains for the areas the commits touched, and on success marks the task done with `bea`. On a conflict it stops with the conflicted files listed; resolve them, `git cherry-pick --continue`, then rerun the script with `--after-conflict` to finish the cleanup and verification.

The backend chain builds into `orchestrator/target-main`, which belongs to the coordinator alone: no subagent worktree ever builds into it, so nothing in it can be a sibling's stale artifact and the chain runs incrementally. Measured on a one-line change in `orchestrator/src/lib.rs`: 36 s of compiling (2.5 s + 6 s clippy, 28 s for the test build) against the 102 s the forced `touch` cost (65 s + 29 s + 8 s). The rest of the chain's six minutes is the 822 tests actually running, which nothing here changes. Never point a worktree at it, and do not set `CARGO_TARGET_DIR` yourself before calling the script; it exports its own. It prints the directory it uses, tees the cargo output to `orchestrator/target-main/merge-chain.log`, and if the merged commits changed anything under `orchestrator/src`, `tests` or `migrations` it fails unless that output contains a `Compiling mars-orchestrator` line — a chain that compiled nothing verified nothing. When the commits touched `orchestrator/migrations/` it also touches `tests/common/db.rs`, `tests/migrations.rs` and `src/main.rs`, because `sqlx::migrate!` embeds the SQL at compile time. Still read the test counts: the assertion proves a build happened, not that the build was the one you wanted.

Do not push after a merge. The local chains the script runs are the gate for each task: they are the same commands the Orchestrator and Frontend workflows run, and with `DOCKER_HOST` set they include the testcontainers and Podman engine tests. `main` stays ahead of `origin/main` for the length of the epic and is pushed once when the epic closes, so CI runs once per epic instead of once per task and nobody waits on a runner between waves. The next wave's worktrees start from local `main` because the dispatch template begins with `git reset --hard main`; keep that line. If a wave has to be checkpointed (the epic spans more than a day, or the user asks), push at the wave boundary and go on dispatching without waiting for the run.

Commit `.bears/` state changes at the end of each wave as `chore(infra): track <epic> progress in Bears (<epic id>)`; the tracker files are part of the repository, and a subagent's branch must never carry them.

## Pitfalls seen in practice

- **Shared cargo target directory — a subagent-side hazard only.** Pointing subagents at `orchestrator/target` through `CARGO_TARGET_DIR` avoids a cold build per worktree, but cargo can reuse a test binary whose compile-time `CARGO_MANIFEST_DIR` points at a deleted worktree, which fails tests that read files relative to the manifest. Worse, two worktrees of the same package can collide so that a subagent's clippy or test run reuses a sibling's artifact and reports "clean" without compiling the subagent's code at all. Seen in the authentication epic: three of thirteen agents ran a sibling's binary at least once (404/405 on their own routes, or the old test count). Keep the shared directory, it saves minutes per task, and keep the `touch` advice in the dispatch template: subagents `touch` their sources before the final run, confirm the `Compiling mars-orchestrator (<their worktree>)` line, check each test binary's count, and touch and rerun rather than suspect the code. The coordinator is immune by construction — its chain builds into `orchestrator/target-main`, which no worktree writes to, so nothing there can be stale and the merge script neither touches sources nor rebuilds from scratch. That immunity is the whole reason the directory is private; if a worktree is ever pointed at it, the hazard comes back and the merge gate stops meaning anything.
- **Tests that spawn the crate binary need a timeout.** The same collision hit `tests/rotate_secrets_cli.rs` in the secrets epic: a sibling's build replaced `target/debug/mars-orchestrator` with a version without the subcommand dispatch, the spawned child started the *server* instead of exiting, and the subagent's full suite hung for forty minutes. Any test that runs `env!("CARGO_BIN_EXE_...")` spawns with `kill_on_drop(true)` under a `tokio::time::timeout` that panics with a clear message, so a stale binary fails loudly.
- **Task bodies predate the code they extend.** Every secrets-epic task was written before the schema epic landed and named methods (`find_by_scope_name`, `update_wrap`, `SecretRow`) that already existed under other names (`find_by_name`, `rewrap`, `Secret`). Before dispatching a wave, grep for the types and methods the task bodies name and write the mapping into the prompt's guidance section ("the task's X is the existing Y; extend, do not duplicate"); a subagent without it either rewrites what exists or invents a parallel API. The mapping also lets you tell the agents to leave the task body's names out of their reports as deviations.
- **No private target directories in the scratchpad.** The session scratchpad under `/tmp/claude-1000/` has a small disk quota; two subagents that built there hit "Disk quota exceeded" mid-build, which also broke their shell's output capture until the directory was deleted. Subagents stay on the shared `orchestrator/target` and verify a real build by the `Compiling` line instead.
- **`ON DELETE RESTRICT` raises SQLSTATE 23001.** Postgres reports a blocked delete as `restrict_violation`, not the `23503` that `sqlx`'s `is_foreign_key_violation()` matches; `repositories::foreign_key_violation` accepts both. A subagent mapping a RESTRICT refusal to 409 needs that helper, not the sqlx predicate alone.
- **Embedded migrations.** `sqlx::migrate!` embeds the SQL at compile time and cargo does not always notice a changed `.sql` file. A subagent that adds or edits a migration must `touch` the files that use the `Migrator` (`tests/common/db.rs`, `tests/migrations.rs`, `src/main.rs`) before testing, or a stale binary reports the old migration count. The merge script does the same three touches for the coordinator's chain whenever the merged commits changed `orchestrator/migrations/`; it is the one touch a private target directory does not make unnecessary.
- **Attribution trailer.** A subagent running as Opus tends to write its own model name into `Co-Authored-By`. The template now says to use the coordinator's line verbatim; if a report still shows a different line, amend the commit message on the branch before merging (`git -C <worktree> log -1 --format=%B | sed ... | git -C <worktree> commit --amend -F -`).
- **Process cleanup.** A `pkill -f <pattern>` also matches the shell running it and any unrelated process with that string in its command line; one such call killed a developer's dev server. Subagents and you: start background servers with their pid recorded, kill by pid, and never `pkill -f` a bare tool name.
- **Port collisions.** Vite silently moves from 5173 to the next free port while Playwright waits on `PLAYWRIGHT_BASE_URL`; set the variable explicitly when 5173 is busy rather than killing whatever holds it.
- **Two tasks editing one file.** Expect conflicts in `src/prelude/mod.rs`, `frontend/package.json`, `Cargo.toml` and `README.md`. The concurrency note in the dispatch prompt keeps them to a few lines; resolve them by hand rather than reordering waves.
- **Bears MCP binding.** The server follows the session's starting directory. If `start_task` says "task not found", you are talking to another repository's tracker; switch to `bea` from the Mars root.

## Closing the epic

When the last task is merged and pushed:

1. Run the complete chains on `main` once more: backend from CLAUDE.md "Code quality" with `DOCKER_HOST` set, `CARGO_TARGET_DIR=$PWD/orchestrator/target-main` (the same private directory the merge script uses, so this run is incremental too) and no `--skip`; frontend including `npm run test:e2e`.
2. `complete_task` on the epic (or `bea done <epic>`), commit `.bears/`, then `git fetch origin` and check `git log main..origin/main`. Another session may have pushed while the epic ran (it happened in the images epic); if so, `git rebase origin/main`, then run the chains again for the areas the incoming commits touched, because the merge gate never saw the combination. Then push `main` once: `git -C ~/dev/mars push origin main`. This is the epic's only push and the only time CI runs for it. When the epic cannot close because its remaining tasks need something only the operator has (real credentials, a manual procedure), push at that point anyway: the merged work must not sit unpushed while it waits, and a workflow's own CI is the one check the local chains cannot run.
3. Wait for the runs on that commit (`gh run list --commit <sha>`, then `gh run watch <id> --exit-status` for each). The Docker half of the Engine workflow is the one check the local chains cannot cover, so it is the run most likely to be new information. A red run is fixed forward on `main` with a `fix` commit that names the task id, pushed again and watched again; it is never left for the next epic.
4. Report to the user: which tasks merged (one line each), what CI says, every deviation from the task texts, every follow-up task filed, anything that could not be verified and why, and the next ready epic.

## Suggesting improvements

This skill records how the scaffolding epic was run. When a wave shows a repeated cost (the same conflict, the same manual verification, the same prompt correction), update the skill or the dispatch template in the same session as a `docs(infra)` commit, so the next epic starts from the improved version.
