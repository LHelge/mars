---
name: implement-epic
description: Work through a Bears epic task by task with `task-implementer` subagents (Opus, medium effort), each in its own git worktree, running the tasks that have no unmet dependencies in parallel. This session dispatches, reviews, merges onto main, runs the quality chains, keeps Bears current and pushes once when the epic closes. Use when the user wants an epic or a batch of Bears tasks implemented ("do the next epic", "start implementing", "work through the ready tasks"), even if they do not mention subagents.
---

# Implement an epic

You coordinate; `task-implementer` subagents write the code. The standing rules for a subagent (worktree setup, build directories, commit format, report format) live in `.claude/agents/task-implementer.md`, so a dispatch prompt is little more than the task body. Do not implement tasks yourself: your context is for reviewing all of them.

## Setup

1. `git status` is clean on `main`. If only `.bears/` is dirty, commit it first as `chore(infra)`. Then `git fetch origin`: if `origin/main` is ahead, rebase onto it before reading Bears, because `.bears/` is only as current as the checkout and another clone may have finished, claimed or pushed the very tasks this one still shows as open. An `in_progress` task with no worktree here is another session's until the user says otherwise.
2. `export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock` for every cargo command you run.
3. `df -h /`. Above 70 % used, `rm -rf orchestrator/target-main` (the next verification rebuilds it) and remove any worktree a finished agent left under `.claude/worktrees/`, before dispatching: cargo never evicts old test binaries, `target-main` grows by tens of GB per epic, and every running agent adds a build directory of its own.
4. Pick the epic with `list_epics` and read it with `get_graph`. If the Bears MCP tools show another project's tasks, the server is bound to the wrong directory; use `bea ... --json` from the repository root instead.
5. Tell the user in one line which epic and how many tasks, then start. Ask only if something in the epic is unclear.

## The loop

Repeat until the epic has no open tasks.

1. **Find the round.** The round is every task of the epic that `list_ready` returns: the tasks whose dependencies are done. If two of them create the same new file, hold one back for the next round.
2. **Dispatch.** For each task: `get_task`, `start_task`, then an `Agent` call with `subagent_type: task-implementer` and `isolation: worktree`. Send the whole round in one message so the agents run in parallel. The prompt:

   ```
   Implement Bears task <id>, "<title>".

   <full task body from Bears, unedited>

   Notes: <only what the body cannot know: files a sibling agent edits in this round ("keep your edit in src/prelude/mod.rs to your own lines"), existing names that differ from the ones the body uses. Omit if empty.>
   Commit trailer: <the attribution lines this session's system reminder gives>
   ```

3. **Review and merge** each branch as its report arrives. Read the diff, not the report (`git log --oneline main..<branch>`, `git diff main...<branch>`): it stays within the task's scope, meets the acceptance criteria, carries the document change rule 1 requires, holds nothing rule 3 forbids and no `.bears/` changes. A small gap you fix yourself after merging; a large one goes back to the agent with `SendMessage` before merging. A real finding outside the task's scope becomes a new Bears task under the epic. Then:

   ```bash
   git cherry-pick main..<branch>        # the branch base is often behind main, so no fast-forward
   git worktree unlock <worktree>; git worktree remove --force <worktree>; git branch -D <branch>
   ```

   Removing the worktree removes the agent's build directory with it (`orchestrator/target` inside the worktree), so do it as soon as the branch is on `main`, not at the end of the epic.

   Conflicts between siblings are usually a few lines in `src/prelude/mod.rs`, `Cargo.toml`, `package.json` or `README.md`; resolve them by hand and `git cherry-pick --continue`.
4. **Verify once per round**, when all its branches are on `main`: run the CLAUDE.md "Code quality" chains for the areas the round touched, the backend chain with `CARGO_TARGET_DIR=$PWD/orchestrator/target-main`. The backend chain ends in three test commands now (ADR 0037): `cargo nextest run`, the doctests and `cargo test --features integration-tests --test engine --test session_e2e`. That last one is the live engine suites, which nextest excludes and which nothing else in the epic runs, so it is part of the round's verification and not optional. That directory is yours alone, and yours is the only run of the whole suite: each subagent builds in its own worktree and runs only fmt, clippy and the test binaries its task concerns, because a full build is about 48 GB per directory. A failure in a binary no agent ran is therefore expected to show up here first. If the round changed `orchestrator/migrations/`, first `touch tests/common/db.rs tests/migrations.rs src/main.rs` in `orchestrator/`, because `sqlx::migrate!` embeds the SQL at compile time. If it changed `.github/workflows/`, run actionlint (`podman run --rm -v $PWD:/repo:ro -w /repo docker.io/rhysd/actionlint:latest`). A failure is fixed forward on `main`: a few lines yourself, anything larger by a new `task-implementer` given the failing output.
5. **Record.** `complete_task` for each task of the round, then commit `.bears/` as `chore(infra): track <epic> progress in Bears (<epic id>)`. Do not push.

## Closing the epic

1. `complete_task` on the epic and commit `.bears/`.
2. `git fetch origin`. If `origin/main` moved, `git rebase origin/main` and rerun the chains for the areas the incoming commits touched.
3. `git push origin main`: the epic's only push, so CI runs once. Watch it (`gh run list --commit <sha>`, `gh run watch <id> --exit-status`); a red run is fixed forward with a `fix` commit naming the task id and pushed again.
4. Report: the tasks merged (one line each), what CI says, deviations from the task texts, follow-up tasks filed, anything that could not be verified, and the next ready epic.

If the same problem costs time in two rounds, fix it in the agent definition or in this file as a `docs(infra)` commit, in one or two lines.
