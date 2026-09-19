---
id: "6y6pw"
title: Read task states through the mutation's connection instead of a second pooled connection
status: open
priority: P2
created: "2026-09-19T13:44:06.958456872Z"
updated: "2026-09-19T13:44:06.958456872Z"
tags:
  - orchestrator
  - tracker
  - architecture
parent: "5h3y4"
---

## Summary
Several tracker functions read `task_states` on the pool while their `TrackerMutation` already holds a pooled connection with the project row locked: `tracker::state` (`state_of`, `lowest_terminal_state`, `resolve_state`), `tracker::leases` (`claim_for_launch`'s non-terminal state list, `human_state`) and `tracker::tasks::create_task` through `resolve_state`. Each therefore needs two connections at once. The reads are correct (only project-locked mutations write `task_states`, and the caller holds that lock), but N concurrent mutations on N projects can each hold one connection and wait for a second, which exhausts a pool of size N and stalls until the acquire timeout.

Found while reviewing `cuw5s` and `cws3a` during the work for `md2zq`.

## Documents
- `ARCHITECTURE.md` "Task tracker" → "One mutation at a time per project" (helpers share the transaction).
- `docs/data-model.md` "Tracker mutation transactions".

## Acceptance criteria
- [ ] `TaskRepository` gains `Locked<'_>`-taking reads for what a mutation needs from `task_states` (`find_state_in`, `find_state_by_name_in`, `list_states_in`), and every read made while a `TrackerMutation` is open goes through `m.conn()`.
- [ ] `grep -n "m.pool()" orchestrator/src/tracker` shows the pool used only to construct a `TaskRepository`, never for a query that runs while the mutation is open; consider whether `TrackerMutation::pool()` can then become `pub(crate)` or go away in favour of the mutation handing out the repository.
- [ ] A test opens as many concurrent mutations on distinct projects as the pool has connections, each performing a `change_state`, and all complete well inside the acquire timeout.
- [ ] The lock-free callers (`resolve_state_in_pool`, list filters, `ready_summaries`) keep reading on the pool.
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed.

## Documentation
- none expected: the documents already say helpers share the transaction; this makes the code do so for reads too.