---
id: zhcgs
title: "Git: revert an integration head to a commit, optionally reopening the tasks merged since"
status: done
priority: P2
created: "2026-09-25T21:04:13.040067790Z"
updated: "2026-09-25T22:47:45.454257258Z"
tags:
  - orchestrator
  - git
  - tracker
  - docs
depends_on:
  - x4st6
  - zhj6q
parent: ny9yq
attempts: 1
---

## Summary

Add `POST /projects/{pid}/git/revert`, which rolls an integration head back to an earlier state by writing **one new commit**, never by resetting the head. It can also reopen the tasks that were merged after that point.

## Request and response

Request:
```
{ branch, to: <full commit id>, expected_head: <full commit id>,
  reopen?: { state: <state name>, comment: string } }
```

Success response: `{ commit, reverted: HistoryEntry[], reopened: Task[] }`.

## Rules

1. `branch` must be an integration head (400 otherwise). `to` must be a first-parent ancestor of the head, strictly older than it (400 otherwise). If `expected_head` does not match the current head, return 409 `branch has moved`, so the confirmation the user saw is the one that is applied.
2. Under the project git lock:
   - Create the commit with `git commit-tree <to>^{tree} -p <head>`. No temporary clone is needed, because no merge takes place.
   - The commit uses the bot identity (`GitCredentialProvider::commit_identity`).
   - Message: `Revert <branch> to <short to>`. The body lists the reverted first-parent commits, followed by a `Requested-By: user:<id>` trailer.
   - Write the head with a compare-and-swap: `git update-ref refs/heads/<branch> <new> <head>`.
3. `reverted` is the first-parent range `to..head`, attributed through the history task's helper.
4. With `reopen`, the same git lock stays held (ARCHITECTURE.md "Git model", Serialization: git lock before the tracker lock). One `TrackerMutation` then takes every task attributed in `reverted` that is in a terminal state. For each one it:
   - moves it to `state` (which must be a queue or human state of the project, 400 otherwise);
   - drops its current hand-off through task x4st6's verb;
   - writes `comment` together with a system line naming the revert commit.

   Tasks not in a terminal state are listed in `reverted`, but are not moved. If the tracker half fails after the ref was written, the revert stays (a failed push never rolls back a merge, and this follows the same rule). Answer 500 with the commit named in the log, and log at `error`.
5. The response does not push. Pushing is the existing push action.
6. No `git` event, because no session is named, following the existing rule for operations that name no session.

## Docs (same commit)

- SPEC.md "Git": the endpoint and its errors.
- ARCHITECTURE.md "Git model": a "Revert" paragraph, including why this is never a reset (ADR 0050: integration heads only move forward).
- A new ADR (next free number in `docs/decisions/`) recording "revert, never reset" and "users only, no MCP", taken from the epic body.

## Testing

- Git tests on real bare repositories: the tree after the revert equals `to`'s tree; the head moves forward; a stale `expected_head` leaves the head untouched; `to` not being an ancestor is refused.
- HTTP tests: the happy path with and without `reopen`; a reopen with a terminal target state (400); 409 on a moved head; unauthenticated and non-member.
- The tracker effects are asserted through the returned tasks and the task events.