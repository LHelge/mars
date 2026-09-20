---
id: sey9x
title: "Write git E2E specs: Changes panel, sync, session branches, merge, rebase, push to the bare upstream and conflict display"
status: in_progress
priority: P1
created: "2026-09-16T20:46:22.448655931Z"
updated: "2026-09-20T18:56:49.355315666Z"
tags:
  - frontend
  - git
  - tests
depends_on:
  - "2acdq"
parent: "6s8j7"
attempts: 1
---

## Summary
Cover the "Git operations" feature paragraph and the session view's "Changes" panel using commits made on the host directly in the session's work clone (the stub executes nothing): sync a session branch into the mirror, view its diff, list session branches with ahead/behind, merge a session branch into `main`, rebase it onto `main`, push `main` to the `file://` upstream and verify with `git`, and see conflicting paths when a merge fails. The GitHub compare link is asserted as absent for a non-GitHub remote and present for a fake `github.com` remote URL rendered client-side.

## Documents
- `SPEC.md` "User-facing features", "Git operations" paragraph.
- `SPEC.md` "Git" table (`GET /projects/{pid}/git/session-branches` → `SessionBranch = { session_id, ref: "refs/sessions/<id>", commit, ahead, behind, base, updated_at }`; `GET .../diff?head=&base=` → `Diff = { base, head, merge_base, files: {path, status, additions, deletions}[], patch, truncated }`; `POST .../merge` `MergeInput = { target, message? } & ({ source } | { task_id, handoff_id })` → `{commit}` or 422 `{status, error, conflicts}`; `POST .../rebase` `{branch, onto}` → `{commit}` or 422; `POST .../push` `{ref, remote_branch?, force?}` → `{remote_branch, commit}`, 409 on non-fast-forward; upstream-tracking refs cannot be mutation targets, 400).
- `SPEC.md` "Sessions" table (`POST /sessions/{id}/sync` → `{ref, commit}`), "Projects" (`GET /projects/{id}/branches`, `POST /projects/{id}/fetch`).
- `SPEC.md` "Frontend", "Changes panel" (fetches diff when opened and on every `git` event; files with counts; same diff renderer; compare link `https://github.com/<owner>/<repo>/compare/<target>...<remote_branch>?expand=1` after a push to a `github.com` remote).
- `ARCHITECTURE.md` "Git model" ("Session clone": `session/<sid>` in the work clone with the user's identity; "Fetch-back"; "Merge, rebase, push" in a temporary clone, conflicts return paths, rebase reconciles the work clone when clean; "Diff" from merge-base, internal fetch-back emits no `git` event; "Commit identity": merge commits carry `GIT_BOT_NAME`/`GIT_BOT_EMAIL` and a `Requested-By: user:<id>` trailer).
- `SPEC.md` "AgentEvent" `git` kind (`op: sync | merge | rebase | push`, `ok`, `detail`).

## Acceptance criteria
- [ ] `frontend/tests/git.spec.ts`; each test: user, bare repo with `README.md` and `src/app.txt`, ready project, a launched stub session waited to `running` (the work clone must exist), ended in `afterEach`; `test.setTimeout(180_000)`.
- [ ] `changes panel shows the session diff after sync`: `commitInSessionWorkClone(sid, { "src/app.txt": "v2\n", "NEW.md": "hi\n" }, "feat: change")`; open the Changes tab → empty until sync; click Sync (or trigger `POST /sessions/{id}/sync`) → the transcript shows a `git` event `sync` ok, the panel lists `src/app.txt` (modified, +1 −1) and `NEW.md` (added), the patch renders with the diff renderer; `gitRevParse(mirrorPath(pid), "refs/sessions/<sid>")` equals the commit.
- [ ] `session branches list ahead/behind`: project git tab lists the session branch with `ahead: 1`, `behind: 0` after the sync above; after `commitToBareRepo(repo, {...}, "upstream")` plus "Fetch now" and merging `origin/main` into `main` from the UI (source `origin/main`, target `main`), the list shows `behind: 1`.
- [ ] `merge session branch into main`: from the session or project git tab: source = session, target `main` → success shows the merge commit; `gitIsAncestor(mirror, sessionCommit, "main")` is true; `git -C mirror log -1 --format=%an%n%b main` shows `Mars E2E Bot` and a `Requested-By: user:<id>` line; the transcript shows a `git` event `merge` ok.
- [ ] `rebase session branch onto main`: after an upstream commit merged into `main`, rebase session onto `main` → new session commit id; the work clone `git -C <work> rev-parse HEAD` equals the new `refs/sessions/<sid>` (clean tree reconciled) and the transcript `git` event `rebase` is ok.
- [ ] `push main upstream`: push `main` → `{remote_branch: "main", commit}`; `gitRevParse(repo.path, "main")` equals the pushed commit; no compare link is rendered for a `file://` remote.
- [ ] `non-fast-forward push is refused`: `commitToBareRepo` on `main` directly, then push `main` again without force → the 409 message is shown and `gitRevParse(repo.path, "main")` is the upstream commit; with `force: true` the push succeeds and upstream equals the mirror head.
- [ ] `merge conflict lists the paths`: upstream commit changing `src/app.txt` to `upstream\n`; fetch; merge `origin/main` into `main`; session commit changing the same file differently; sync; merge session into `main` → 422; the UI lists `src/app.txt` under conflicts; `main` is unchanged (`gitRevParse` before/after).
- [ ] `upstream-tracking ref as a mutation target is rejected`: merge with target `origin/main` (if the UI allows typing it; otherwise via the helper API) → 400 shown.
- [ ] `github compare link is built client-side`: create a project whose `remote_url` is `https://github.com/example-org/example-repo.git` (it will reach `error` or stay `cloning`, that is fine: use the API to check the frontend's link builder in isolation by rendering the push result for the real project) — if this cannot be exercised without a real remote, replace by a Vitest/unit test note in the coverage table and `test.skip` here with the reason.

## Implementation notes
- Files: `frontend/tests/git.spec.ts`.
- The work clone at `<dataDir>/sessions/<sid>/work` is on branch `session/<sid>` after launch; commits made there by the host are what `sync` fetches back. The stub session is `running` throughout; ending it also syncs (fetch-back on end), which the lifecycle spec asserts.
- Upstream commits go through `commitToBareRepo` (helper; temporary clone of the bare repo, commit, push).
- Assert `git` events through the transcript rows rather than the WebSocket; the Changes panel refreshes on them.
- `git log` assertions run on the mirror path on the host via `execFileSync("git", ["-C", mirror, ...])`.

## Edge cases
- The internal diff fetch-back must not emit a `git` event: after opening the Changes tab twice, the transcript shows exactly one `sync` event (from the explicit Sync click), not more.
- A rebase when the work tree is dirty reports "checkout reconciliation is required" in the `git` event; create that case by leaving an untracked-and-modified tracked file in the work clone and assert the event text, then clean up.
- Podman keep-id: files the host writes into the work clone are owned by uid 1000 in the container; nothing in these specs runs inside the container.

## Testing
- The spec file; run twice against one stack.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Frontend project and session views": Changes panel, git actions on the project page (session branches, merge, rebase, push, conflict display, compare link), Sync control on the session.
- "Git operations: mirror, clones, integration and REST API": every `/git` route, sync on `/sessions/{id}/sync`, `file://` upstream push without a credential.