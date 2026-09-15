# 0001. Session clones instead of git worktrees

Status: accepted

## Context

Each session needs its own writable checkout, mounted into an isolated container that must be able to commit. `git worktree` is the obvious space-saver: one object store, many checkouts. But a worktree's `.git` is a file pointing into the main repository's `.git/worktrees/<name>`, and commits write to the main repository. A container that only sees the worktree directory cannot commit, and giving it the main repository read-write breaks isolation between sessions.

## Decision

Every session gets a full clone at `/data/sessions/<id>/work`, created with `git clone --reference <mirror>` so that objects are borrowed from the project mirror through `.git/objects/info/alternates`. The mirror is mounted read-only into the container at the same absolute path as on the host so the alternates path resolves. The work directory is mounted read-write.

The mirror is configured with `gc.auto=0` and `gc.pruneExpire=never` so that objects a session clone borrows are never deleted underneath it. Session branches are integrated back into the mirror by the orchestrator (see ADR 0007), never by the container.

## Consequences

- Sessions are fully isolated: a corrupted or deleted session clone affects nothing else.
- Disk cost per session is the checkout plus new objects only; the history is shared through alternates.
- The mirror must never be repacked or pruned in a way that drops objects; this is enforced by config and must be respected by any maintenance job.
- `git clone --reference` requires the path to be identical inside and outside the container. `/data` is therefore mounted at `/data` everywhere.
