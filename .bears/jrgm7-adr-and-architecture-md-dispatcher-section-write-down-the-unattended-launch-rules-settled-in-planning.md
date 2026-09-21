---
id: jrgm7
title: "ADR and ARCHITECTURE.md \"Dispatcher\" section: write down the unattended-launch rules settled in planning"
status: open
priority: P1
created: "2026-09-21T20:07:29.698636172Z"
updated: "2026-09-21T20:23:25.141734215Z"
tags:
  - docs
  - adr
  - dispatcher
  - orchestrator
parent: qabvt
---

## Summary
The rules were decided with the user while planning (2026-09-21) and are listed in the epic `qabvt`, "Decisions". This task is the write-up, before any code: a new ADR in `docs/decisions/` and a real "Dispatcher" subsection in `ARCHITECTURE.md`, "Task tracker", replacing the dispatcher bullet of "After v1: dispatcher and scheduled agents". Documents only. Do not reopen the decisions; a rule that turns out not to be writable as decided goes back to the user.

## Acceptance criteria
- [ ] ADR `docs/decisions/00NN-…` (next free number, listed in `docs/decisions/README.md`) records each decision with the alternative it rejected: the three caps counting all live sessions and binding automation only (rejected: counting dispatcher-launched sessions only; an instance cap alone); the project-level `automation_paused` switch (rejected: an instance `DISPATCHER_ENABLED` variable; the profile flag alone); `auto_launch` refused at save without a `global`/`project` agent credential, with the skip-and-log backstop (rejected: launch and let it fail); the attempt limit as the only back-off (rejected: a per-profile breaker; `creating` failures not counting as attempts); ephemeral only; served states honoured; `ready_summaries` order; older profile wins; `sessions.launch_source` as the record of who launched (rejected: inferring automation from `created_by IS NULL`, which a deleted user also produces).
- [ ] `ARCHITECTURE.md`, "Task tracker" gains "Dispatcher" with the rules in the present tense, marked as not yet implemented until the job task lands if the document's conventions need that; "Launching a session for a task" says a dispatcher launch is the same path with no user and with served states enforced. The scheduled-agents half of the sketch stays a sketch (`tup8z` replaces it).
- [ ] The term *unattended launch* and the capacity rule are defined once, in a place both the "Dispatcher" and the later "Scheduled agents" sections can point to.
- [ ] `docs/data-model.md`, `agent_profiles` closing paragraph ("columns on this table and one background job") is corrected: two columns land on `projects` as well.
- [ ] No code changes. `SPEC.md`, `README.md` configuration and the data-model tables change with the tasks that add the columns and the variable, not here.