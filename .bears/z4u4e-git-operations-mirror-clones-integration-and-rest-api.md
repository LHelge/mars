---
id: z4u4e
title: "Git operations: mirror, clones, integration and REST API"
type: epic
status: open
priority: P1
created: "2026-09-16T20:12:40.916965835Z"
updated: "2026-09-16T20:15:18.877232109Z"
tags:
  - orchestrator
  - git
depends_on:
  - t36d2
---

## Scope

`git/`: everything in `ARCHITECTURE.md`, "Git model", shelling out to the `git` binary (ADR 0011), plus the git REST endpoints.

- Command wrapper with argv arrays, structured error parsing, and `GitCredentialProvider` (ADR 0002): PAT from the project `GIT_CREDENTIAL` secret written as `http.extraHeader` into a `0600` temp config selected via `GIT_CONFIG_GLOBAL`, deleted afterwards; `commit_identity` from `GIT_BOT_NAME`/`GIT_BOT_EMAIL`; `secret_uses` rows with `purpose = git`.
- Project repository: `init --bare`, `origin` with the documented refspecs, `gc.auto=0`, `gc.pruneExpire=never`, symbolic `HEAD` discovery, seeding integration heads after the first fetch, subsequent fetches touching only upstream-tracking refs and tags (ADR 0017).
- Session clone (`--reference --no-checkout`, explicit fetch of the selected ref, `session/<id>` branch, launching user's identity), fetch-back into `refs/sessions/<sid>`, base-ref resolution for heads, upstream refs, tags, session refs and commit ids.
- Temp-clone merge and rebase in `/data/tmp` with explicit ref fetches and write-back, conflict reporting with paths, work-clone reconciliation after rebase; push with explicit refspec, non-fast-forward as conflict, force only when requested; hand-off ref retention `refs/handoffs/<id>` and removal.
- Per-project asynchronous git lock with the documented ordering (git lock before any database lock).
- Diff: `--numstat` and patch from `merge-base`, session heads synced first without a `git` event, `handoff_id` selection, 1 MiB truncation.
- REST: `GET /projects/{pid}/git/session-branches`, `GET .../diff`, `POST .../merge` (branch form and task form; the task form's tracker checks are wired in the Code hand-offs epic), `POST .../rebase`, `POST .../push`, with 422 conflict bodies and 409 for non-fast-forward, and the `git` outcome events on the session.
- Mirror fetch routine reused by the cron job and `POST /projects/{id}/fetch`.

## Documents

`ARCHITECTURE.md` "Git model", "Known v1 vulnerability"; `SPEC.md` "Git (`/api/projects/{pid}/git`)", "Git operations"; ADRs 0001, 0002, 0007, 0011, 0017, 0019.

## Acceptance criteria

- [ ] Git tests use real bare repositories in `tempfile` directories and cover: init and seeding, fetch preserving an unpushed integration merge, clone from each base kind, fetch-back, merge success and conflict, rebase success and conflict with work-clone update, push success, non-fast-forward and force, diff truncation.
- [ ] Credential never appears in argv or in the stored `remote_url`; the temp config file is removed even on failure.
- [ ] Every git endpoint has happy-path, 400 (bad ref kind), 401, 409 and 422 tests.

## Out of scope

Isolation of git on agent-controlled checkouts (ADR 0019, roadmap).