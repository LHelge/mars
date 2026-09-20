---
id: "2k43n"
title: "Give each task-implementer its own cargo target directory: the shared orchestrator/target made parallel agents' full-suite results unreliable in both directions"
status: open
priority: P2
created: "2026-09-20T10:52:53.752543929Z"
updated: "2026-09-20T10:52:53.752543929Z"
tags:
  - infra
  - tests
  - docs
---

## Summary
During epic qgj33 (2026-09-20) parallel `task-implementer` agents sharing `CARGO_TARGET_DIR=orchestrator/target` linked test binaries against each other's `mars-orchestrator` rlib. Observed: three agents saw all of their own new tests fail with the sibling's stub handlers until they touched every source file; agent pg6ga reported a green full suite although its branch made `tests/tasks_api.rs::each_parent_rule_is_refused_with_its_own_message` fail (a REST 400 became 404; fixed forward in 8826f81's successor on main), because its `tasks_api` binary had been linked against sibling 88zm4's rlib; and 88zm4 in turn saw that same failure in two of its runs and reported it as a load flake, although its diff did not touch the code. The touch-everything guard in `.claude/agents/task-implementer.md` does not help when a sibling rebuilds between an agent's build and its test run. Only the coordinator's private `target-main` run caught the truth.

The same day the two target directories grew to 182 GB and 189 GB and filled the disk.

## Documents
- `.claude/agents/task-implementer.md` ("Stale builds"), `.claude/skills/implement-epic/SKILL.md` (step 4, "That directory is yours alone").
- Epic yq6c3 (the private coordinator target directory) for the reasoning that led to the shared one.

## Acceptance criteria
- [ ] Each agent builds in a target directory of its own that disappears with its worktree (the cargo default inside the worktree, or `target-<task id>` removed by the coordinator's cleanup line), and the "Stale builds" paragraph is replaced by that rule.
- [ ] The skill's cleanup step removes the directory, and the setup step checks `df` and clears `target-main` when the disk is above a stated threshold.
- [ ] If per-agent full builds are too slow or too large, the alternative is written down instead: agents run only fmt, clippy and their own test binaries, and the coordinator's full run stays the single authority.

Discovered from qgj33.