---
id: zhj6q
title: "Git: the default branch's first-parent history, attributed to tasks and sessions"
status: open
priority: P2
created: "2026-09-25T21:03:46.427805809Z"
updated: "2026-09-25T21:03:46.427805809Z"
tags:
  - orchestrator
  - git
  - docs
parent: ny9yq
---

## Summary

Add `GET /projects/{pid}/git/history?branch=<integration head>&before=<commit>&limit=<n>` returning `HistoryEntry[]`: the first-parent history of an integration head, newest first. `branch` defaults to the default branch, `limit` defaults to 50 with a maximum of 200, and `before` is a cursor for pagination. Each entry says which task and session produced it.

## Shape (add to SPEC.md "Git")

```
HistoryEntry {
  commit, parents: string[], subject, author_name, committed_at,
  requested_by: string | null,     // the Requested-By trailer, if any
  tasks: { id, number, title, handoff_id }[],
  sessions: { id, title }[]
}
```

## Attribution

- A merge commit's non-first parents, and a fast-forwarded range between two first-parent entries, are matched against `task_handoffs.commit` of the project. Each match names the hand-off's task.
- Session attribution uses `task_handoffs.source_session_id`, plus the `Requested-By: session:<id>` trailer.
- A commit matching nothing has empty lists. Fast-forwards are why the range matters: an auto-merge that fast-forwarded leaves no merge commit, so the entry for the hand-off tip is itself the hand-off commit.
- Put the attribution in one helper in `git/` or `git/service.rs`, returning commit → hand-off ids, with the DB lookup in a repository. The rollback task reuses it to list the tasks merged after a point.

## Implementation Notes

- Read the refs under the project git lock, as `list_session_branches` does, so the listing describes one moment. `git log --first-parent -z --format=...` directly against the mirror; no temporary clone. Every invocation uses `--end-of-options` (ARCHITECTURE.md "Git model", option injection), and the command must work on git 2.39.
- Read-only and silent: no `git` event.
- `branch` must be an integration head (400 otherwise). An unknown `before` is 400.

## Docs (same commit)

SPEC.md "Git" gets the endpoint and the type; ARCHITECTURE.md "Git model" gets one paragraph, "History".

## Testing

- Git tests on real bare repositories: first-parent walk, pagination, merge-commit attribution, fast-forward attribution, a commit with no hand-off.
- HTTP tests: the happy path, a non-head branch (400), unauthenticated, and non-member.