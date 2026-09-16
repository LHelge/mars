---
id: ku8up
title: Extend tests/utils/test-helpers.ts with API, bare-repository, project, session, task and logged-link helpers
status: open
priority: P0
created: "2026-09-16T20:41:40.471303092Z"
updated: "2026-09-16T20:41:40.471303092Z"
tags:
  - frontend
  - tests
depends_on:
  - arsch
parent: "6s8j7"
---

## Summary
Reshape the skeleton's `frontend/tests/utils/test-helpers.ts` into the helper layer every scenario uses: typed REST helpers that talk to the real orchestrator through Playwright's `APIRequestContext`, browser login helpers (UI and token-seeded), a local bare-repository factory that gives projects a `file://` upstream, a project factory that waits for `ready`, session/task factories with state waits, a host-side commit helper for session work clones, and a reader for the invite and reset links the orchestrator logs. Scenarios stay short and read like the SPEC paragraphs they cover.

## Documents
- `CLAUDE.md` "Testing expectations", Frontend E2E (fresh users per test through `/api/test/users`, helpers in `tests/utils/test-helpers.ts`).
- `SPEC.md` "Test-only routes" (`POST /test/users` `{username, email, password, admin?}` → 201 `{user, access_token}` + refresh cookie); "Authentication" (bearer header, refresh cookie name `refresh_token`); "Projects" (`POST /projects` `{name, remote_url, default_branch?, credential?}` → 201 `status: cloning`; `GET /projects/{id}` → `status` `ready`/`error`); "Sessions" (`POST /projects/{pid}/sessions` `{profile_id, base_ref?, title?, message?, task_id?}` → 201 `state: creating`; `GET /sessions/{id}`; `POST /sessions/{id}/input`, `/stop` → 202, `/end`, `/sync` → `{ref, commit}`); "Agent profiles" (`GET /projects/{pid}/profiles`, `PUT` with `ProfileInput`); "Tasks" (`POST /projects/{pid}/tasks`, `PUT`, `GET /projects/{pid}/tasks/{id}` accepting UUID or number); "Secrets" (`POST /secrets` `{scope, scope_id?, name, value, orchestrator_only?}`); "Users" (`POST /users/invites` → `Invite`, token only in the email/log).
- `ARCHITECTURE.md` "Git model" ("Project clone": `git ls-remote --symref origin HEAD` discovers the default branch; "Session clone": work clone at `DATA_DIR/sessions/<sid>/work` on branch `session/<sid>` with `user.name`/`user.email` set), "Storage" (`DATA_DIR` layout), "Session image" (stub knobs are environment variables; the launcher's environment is `HOME`, `CLAUDE_CONFIG_DIR`, `MARS_*`, then the resolved secrets).
- `SPEC.md` "Users" paragraph on `LogEmailClient` (ADR 0026): full link logged at `info`.

## Acceptance criteria
- [ ] Environment accessors: `apiBaseUrl` (`PLAYWRIGHT_API_URL`), `orchestratorLogPath`, `dataDir`, `reposDir`, `stubImage`, each throwing a clear error naming the variable and `npm run test:e2e:up` when unset.
- [ ] `createTestUser(request, opts?: { admin?: boolean; prefix?: string })` returns `{ id, username, email, password, access_token, refresh_cookie }`; usernames are `e2e-<prefix>-<8 hex>`, emails `<username>@example.test`, password `E2e-password-1234` unless overridden; the refresh cookie is taken from the 201 response's `set-cookie` header.
- [ ] `api(request, access_token)` returns a small typed wrapper (`get/post/put/patch/delete(path, body?)`) that sets `Authorization: Bearer`, prefixes `/api`, throws with status and body on non-2xx unless `{ allow: [409] }` is passed, and parses JSON when present.
- [ ] `login(page, username, password)` drives `/login` and waits for `/`; `loginViaToken(context, user)` seeds the browser without the form: adds the `refresh_token` cookie for the base URL domain and installs the access token where `src/services/auth.ts` keeps it in `localStorage` (import the exported storage key constant; if the foundation epic does not export one, add the export there, one line) via `context.addInitScript`; `newLoggedInPage(browser, user)` creates a context, seeds it and returns the page (used for the second-browser scenarios).
- [ ] `createBareRepo(name, opts?: { files?: Record<string,string>; branch?: string })` creates `<reposDir>/<name>-<hex>.git` with `git init --bare -b main`, pushes one initial commit (`README.md` plus any `files`) from a temporary non-bare clone, sets the bare `HEAD` to `refs/heads/main`, and returns `{ path, url: "file://" + path, initialCommit }`; `commitToBareRepo(repo, files, message)` adds a commit on `main` upstream (via a temp clone) and returns its id, for fetch/merge/conflict scenarios.
- [ ] `createProject(api, { name?, remote_url, default_branch? })` posts, then polls `GET /projects/{id}` every 500 ms until `status` is `ready` or `error` (timeout 60 s), returning the `Project` and throwing with `status_message` on `error`; `defaultProfile(api, projectId)` returns the profile with `is_default: true`; `setProfileSecrets(api, project, names)` PUTs the default profile with `secrets: names`, and `setProjectSecret(api, projectId, name, value)` creates a project-scoped secret (`scope: "project", scope_id`), together the way scenarios pass `MARS_STUB_LINE_DELAY_MS`, `MARS_STUB_EXIT_AFTER_TURNS`, `MARS_STUB_EXIT_CODE` and `MARS_STUB_FIXTURE` into the stub container (secrets are injected as environment variables).
- [ ] `launchSession(api, projectId, { profile_id?, message?, task_id?, base_ref?, title? })` posts and returns the `Session`; `waitForSessionState(api, sessionId, state | state[], timeoutMs = 60_000)` polls `GET /sessions/{id}` and returns the session or throws with the last observed `state` and `error`; `sendInput(api, sessionId, text)` posts `{ kind: "message", text }` and expects 202.
- [ ] `createTask(api, projectId, { title, description?, state?, priority?, labels?, parent_id?, depends_on? })`, `getTask(api, projectId, idOrNumber)`, `moveTask(api, projectId, id, state)` (PUT `{state}`) exist.
- [ ] `commitInSessionWorkClone(sessionId, files, message)` writes files into `<dataDir>/sessions/<sid>/work`, runs `git add -A && git commit -m` there with `-c user.name=E2E -c user.email=e2e@example.test` fallbacks, and returns the full 40-hex commit id; `mirrorPath(projectId)` returns `<dataDir>/projects/<pid>/repo.git`; `gitRevParse(repoPath, ref)` and `gitIsAncestor(repoPath, commit, ref)` wrap `git -C`.
- [ ] `logOffset()` returns the current byte length of the orchestrator log; `readLoggedLink(kind: "invite" | "reset-password", email, sinceOffset)` polls the log for up to 15 s for text after `sinceOffset` that contains the email and then the first `http://localhost:5173/(invite|reset-password)/<token>` occurrence after it, and returns the URL; tokens are matched with `[A-Za-z0-9._~-]+`.
- [ ] `uniqueName(prefix)` and `waitFor(fn, { timeoutMs, intervalMs })` utilities exist; all helpers are typed against `frontend/src/types/` (import the API types rather than redeclaring them).
- [ ] The skeleton's `smoke.spec.ts` still passes; `npm run lint && npx tsc -b` pass with `tests/**` type-checked.

## Implementation notes
- Files: `frontend/tests/utils/test-helpers.ts` (grow, or split into `tests/utils/api.ts`, `tests/utils/git.ts`, `tests/utils/log.ts`, `tests/utils/browser.ts` re-exported from `test-helpers.ts` so the documented path stays valid).
- Git commands run with `child_process.execFileSync("git", [...], { cwd })`, never through a shell string; set `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1` for determinism, and `GIT_TERMINAL_PROMPT=0`.
- Project `remote_url` is `file:///abs/path/x.git`; the orchestrator runs on the same host (stack task), so its `git ls-remote` and fetch work without a credential. Session containers never see the upstream; they clone from the mirror.
- Polling helpers log nothing on success and include the last response body in thrown errors so a failing CI run is diagnosable from the Playwright report.
- The log reader must tolerate multi-line `text=` fields (the email body contains newlines) and both compact and pretty `tracing` formats: search for the email first, then the link, both after `sinceOffset`.

## Edge cases
- `POST /test/users` 409 on a duplicate username cannot happen with random suffixes, but the helper still surfaces the body.
- `createProject` reaching `error` because the engine or git is misconfigured must fail fast with `status_message`, not time out.
- `commitInSessionWorkClone` while the session container is running: the clone is bind-mounted read-write; writing from the host is safe for the stub, which never touches the tree. Files created by the host user are owned by the host uid, which under keep-id is uid 1000 in the container.
- `readLoggedLink` for a user whose invite was re-sent: the caller passes a fresh `sinceOffset` taken before the resend.

## Testing
- `frontend/tests/helpers.spec.ts`: creates a user and calls `GET /users/me`; `loginViaToken` then `/` renders the dashboard; `createBareRepo` yields a repository whose `git rev-parse main` equals `initialCommit`; `createProject` reaches `ready` and `GET /projects/{id}/branches` lists `main` and `origin/main`; `readLoggedLink` finds an invite link after `POST /users/invites` as an admin.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e`.

## Documentation
- none: implements the documented contract as written (helper conventions are internal to `frontend/tests/`).

## Assumes from other epics
- "Repository scaffolding, tooling and CI": the skeleton helper signatures `createTestUser`, `login`, `apiBaseUrl`.
- "Frontend foundation": `src/services/auth.ts` exports the `localStorage` key of the access token; `src/types/` mirrors `SPEC.md`.
- "Git operations": `POST /projects` accepts `file://` remotes and discovers `main` through `ls-remote --symref`.
- "Secrets manager" and "Session lifecycle": profile-declared secrets are injected as environment variables into the session container, so `MARS_STUB_*` knobs reach the stub.