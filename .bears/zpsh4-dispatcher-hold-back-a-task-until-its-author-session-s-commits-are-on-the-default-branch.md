---
id: zpsh4
title: "Dispatcher: hold back a task until its author session's commits are on the default branch"
status: done
priority: P1
created: "2026-09-25T21:00:29.504761964Z"
updated: "2026-09-25T21:23:26.735838387Z"
tags:
  - orchestrator
  - dispatcher
  - git
  - docs
parent: "4txx2"
attempts: 1
---

## Summary

Implements the epic's rule in the dispatcher (`ARCHITECTURE.md`, "Dispatcher"). Before launching for a candidate task whose `created_by_session_id` is set, the dispatcher checks whether that author session's work has landed. If it has not, the candidate is skipped with a fixed-word reason (`author_work_unlanded`) and the dispatcher moves on to the next one. User launches, MCP `claim`, and auto-merge are **not** gated: a person decides for themselves, and an agent that claims over MCP was pointed at the task by someone.

## The rule ("landed")

The author session's work has landed when any of these holds:
- `created_by_session_id` is NULL. Either no session wrote the task, or that session has been deleted.
- The session has no work tree and no ref (`GitError::UnknownRef` from the sync). It produced nothing, or ended with no unique work (ADR 0050).
- Its tip equals `sessions.base_commit`, meaning it made no commits.
- Its tip is an ancestor of, or equal to, the project's default integration head.

Otherwise the work is unlanded. Note that a tip held only by a hand-off ref is still **unlanded**: a hand-off is not the default branch.

## Implementation Notes

- Add a method to `GitService` (`orchestrator/src/git/service.rs`), for example `author_work_landed(&self, guard: &ProjectGitGuard, session_id, default_branch) -> Result<bool>`, built from existing parts:
  - `sync_tip(guard, session_id, Phase::FromState)` or `sync_session_silent`. Use the silent path so no `git` event is written, because an event would wake the waker and loop. This also refreshes `refs/sessions/<sid>` for a live session, which is what the board task reads.
  - `base_commit` through `SessionRepository::fetch_back_basis`.
  - `integrate::is_ancestor`, as `is_merged_into` does. A tip the project repository does not have is not an ancestor, and that is fine because the sync fetched it.
- The mock git service behind `integration-tests` needs the matching behaviour. Follow how the mock handles `is_merged_into` and sync.
- In `cron/dispatcher.rs` `dispatch_profile`:
  - Cache the answer per author session for the whole run (a `HashMap<Uuid, bool>` passed down from `dispatcher()`), because many tasks share one planner.
  - Take the project git lock (`self.state` git locks, as `auto_merge.rs` does) only around the check, and release it before `create_session`, which takes the lock itself.
  - `ready_summaries` may not carry `created_by_session_id`. If not, add it to the summary row (`tracker/leases.rs` and its query), then run `cargo sqlx prepare` and commit `.sqlx/`.
  - A git failure during the check is a `failures += 1` with an `error!` log. The task is skipped and never dispatched, since failing closed is the point.
  - Log the skip at `debug` with `task_id` and `author_session_id` fields, and count it in `skipped`.
- The waker already runs on `task_events` and session end. A merge into the default branch writes a `git` event and task events only for task merges. A plain branch merge of the planner's session writes a `git` event on that session, which is a `session_events` notice. Check whether that wakes the dispatcher. If it does not, the 60 s timer picks the task up, which is acceptable; document whichever is true.

## Docs (same commit)

- In `ARCHITECTURE.md` "Dispatcher", add a paragraph "**A task waits for its author's work.**" with the rule above, and why only the dispatcher applies it.
- Add a new ADR in `docs/decisions/` (next number) recording the rejected alternatives from the epic body.
- If `SPEC.md` says anything about dispatcher eligibility, align it.

## Testing

- Unit or integration tests on the git service method with real bare repositories (git is never mocked in git tests). Cover: no commits (tip == base), commits ahead (not landed), after a merge into the default branch (landed), a session with no work tree and no ref (landed), and a tip held only by a hand-off ref (not landed).
- Dispatcher integration tests (existing dispatcher suite, `TestApp`) cover: a task authored by a session with unmerged commits is not dispatched; after the merge it is; a task with no author session is dispatched as before; and two tasks by the same author cost one check. Assert the last through the mock if practical, otherwise skip it.
- Run fmt, both clippy invocations, and the nextest binaries for git and dispatcher.