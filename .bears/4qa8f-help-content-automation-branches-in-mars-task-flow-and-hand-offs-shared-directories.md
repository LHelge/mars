---
id: "4qa8f"
title: "Help content: automation, branches in Mars, task flow and hand-offs, shared directories"
status: open
priority: P2
created: "2026-09-22T19:08:28.070395061Z"
updated: "2026-09-22T19:08:55.718018330Z"
tags:
  - frontend
  - docs
depends_on:
  - ujccg
  - u9jp4
parent: gtbp5
---

Write the Markdown for these `src/help/` topics, same rules as the sibling content task (agree with the docs; fix a wrong doc in the same commit).

- `automation`: auto-launch picks the task the `ready` tool would offer; the three caps (profile `max_concurrent`, project `max_concurrent_sessions`, instance-wide `DISPATCHER_*` set by the operator); pause in project settings; schedules run in UTC, a missed or refused tick is spent, not caught up; unattended launches need a project/Everyone agent credential. Source: SPEC "Automatic dispatch", "Scheduled agents"; ARCHITECTURE "Unattended launches", "Scheduled agents"; ADR 0042.
- `branches`: integration heads (Mars's `main`, what sessions start from and merges land in) vs upstream-tracking (`origin/*`, what Fetch updates) vs session refs; Sync copies a session's committed work into the mirror (uncommitted excluded); to take upstream changes merge `origin/main` into `main`, then push; push uses the project git credential and GitHub shows a compare link; rebasing a running session's branch leaves its work tree to reconcile; a branch merge grants no task approval. Source: README operating notes (git fetch bullet), ARCHITECTURE "Git model", SPEC "Git", ADR 0017.
- `task-flow`: states and their kinds, one human state, no in-progress column (a held task shows its session), leases, attempts and `max_attempts` escalation to the human state, moving a task resets attempts, blocked = open children or unmet `blocks`, escalation email to the assignee else admins; hand-offs: publish a revision (sync first; full commit id equal to the branch tip), approve / request changes, a new revision resets review, task merge merges the approved commit not the branch tip. Source: SPEC "Tasks", "Task board", "Code hand-offs and review" (reuse its worked example); ARCHITECTURE "Task tracker".
- `shared-directories`: why (one build cache instead of one per session), the ecosystem share/keep table, Cargo's lock makes `target` safe while `node_modules`/virtualenvs are not, path rules (absolute, not `/data`, not an ancestor of `/session/work|home|log`, may lie inside `/session/work`), clear/remove refused while a session runs. Source: README operating notes (shared directories bullet and table), SPEC "Shared directories", ADR 0015. Uses the corrected Cargo registry path from the preset fix task.