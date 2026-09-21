# The end-to-end suite

Playwright against a real stack: a Postgres container, the stub session image,
an orchestrator built with `--features integration-tests` and a Vite dev server
Playwright starts itself. Every scenario runs actual session containers; nothing
here is mocked.

`README.md`, "Development" → "End-to-end tests" is the operating manual — the
stack script, the `E2E_*` knobs and what `up` writes. This file is about the
suite: how to run it, how it is arranged, and what it covers.

## Running it

```bash
cd frontend
npm run test:e2e:up          # postgres, the stub image, the orchestrator
npm run test:e2e             # the suite
npm run test:e2e:down        # stops everything and removes the run's state
npm run test:e2e:status      # what is up right now
```

`test:e2e` is wrapped by two checks of the coverage table below:
`pretest:e2e` verifies that every scenario the table names exists, and
`posttest:e2e` verifies that the run skipped nothing the table does not
document. Both are `node tests/coverage-check.mjs`.

**A full run wants a fresh stack.** Two things accumulate on a stack the suite
has already run against, and both are deliberate product behaviour rather than
anything the suite can arrange around:

- The seeded `admin`/`changeme` account is spent by the seeded-administrator
  scenario, so that scenario skips on the second run against the same stack.
- The login throttle blocks the client address after 10 failed attempts in 15
  minutes (`SPEC.md`, "User-facing features" → "Login and invites"). A run
  makes three deliberate failures — a wrong password, a deleted user, and the
  replaced password after a reset — and a *re*run makes a fourth, because the
  probe that decides whether the seeded account is still fresh is a failed
  login too. At about seven minutes a run, the third consecutive run against
  one stack crosses the limit and every login in it, right or wrong, is
  answered 429.

So: two consecutive runs against one stack are fine, three are not. Bring the
stack down and up between full runs — the throttle lives in the orchestrator's
memory, so restarting it clears every counter — and a rerun without that is
still useful for one spec file or one scenario:

```bash
npx playwright test tests/tasks.spec.ts
```

Nothing else in the suite cares: every other scenario makes its own user,
project and repository.

A single scenario, by title:

```bash
npx playwright test -g "interject mid-turn"
```

## Wall-clock

| Run | Machine | Time |
| --- | --- | --- |
| 95 scenarios, warm stack, `workers: 1` | Arch Linux developer machine, rootless Podman 6, 2026-09 | 6.8–7.1 minutes |

CI's `timeout-minutes: 45` covers the suite plus the stack build, with room to
spare.

The three slowest scenarios, and why:

| Scenario | Time | Bound by |
| --- | --- | --- |
| `sessions.spec.ts` › `interject mid-turn` | ~47 s | `MARS_STUB_LINE_DELAY_MS=200` pacing two fixture turns (58 and 159 lines) so the first one is still streaming while the interjection is typed |
| `sessions.spec.ts` › `an idle session is parked without user action` | 33–46 s | the idle reaper's period, a hard-coded 60 s (`orchestrator/src/cron/mod.rs`, `REAPER_PERIOD`). Making it a configuration variable the stack could set to a few seconds is the only thing that would shorten this |
| `session-view.spec.ts` › `older history loads on scroll-up` | ~27 s | the session has to commit more than `PAGE_SIZE` = 200 events before a reload can leave any behind (`src/session/useSessionSocket.ts`). A build-time or runtime page size would let the scenario need a fraction of them |

## How a scenario is arranged

Everything a scenario needs comes from `tests/utils/`, re-exported from
`tests/utils/test-helpers.ts`; `test` and `expect` come from
`tests/utils/fixtures.ts`.

```ts
import { expect, test } from "./utils/fixtures";
import { loginViaToken, waitForSessionState } from "./utils/test-helpers";

test.use({ repoFiles: { "src/app.txt": "v1\n" } });

test("…", async ({ page, context, user, api, project, sessions }) => {
  await loginViaToken(context, user);
  const session = await sessions.launch(api, project.id, { base_ref: "main" });
  await waitForSessionState(api, session.id, "running");
  // …
});
```

| Fixture | What it is |
| --- | --- |
| `user` | a fresh user through the test-only route (`SPEC.md`, "Test-only routes") |
| `api` | a REST client authenticated as `user`, whose user scope is given a fake agent credential so the launch forms take their ordinary path. A second client — an admin, a reviewer — is `apiClient(request, token)` |
| `repo` | a local bare upstream with one commit on `main`, plus whatever `repoFiles` declares |
| `project` | a project cloned from `repo`, waited to `ready` |
| `sessions` | the scenario's sessions. `sessions.launch(...)` launches one; `sessions.track(client, id)` registers one the UI launched. Both are ended when the scenario finishes, so no container outlives a run |

Fixtures are lazy: a scenario that names none of them creates none, which is
why `smoke.spec.ts` needs no stack at all.

Two options are set per spec file with `test.use`: `repoFiles`, above, and
`agentCredential`. The launch forms warn when the caller has no agent
credential and rename their button to `Add credential` (`SPEC.md`, "Frontend",
Agent credentials), so `api` gives its user an obviously fake one and every
scenario exercises the common path. A spec that is about the empty state —
`secrets.spec.ts`, which creates the first credential itself — opts out with
`test.use({ agentCredential: false })`.

The other helper modules: `env` (the stack's connection facts and `waitFor`),
`api` (REST and `createTestUser`), `git` (bare repositories, work clones and
two read-only queries), `log` (the invitation and reset links `LogEmailClient`
writes, ADR 0026), `browser` (`loginViaToken`, `newLoggedInPage` and the socket
drops), `resources` (projects, profiles, secrets, sessions and tasks),
`engine` (`sessionContainers`), `transcript` (`reveal`, `pinToLatest`,
`rowCount`) and `test-ids` (the `data-testid` constants, re-exported from
`src/utils/testIds.ts` so a rename fails `tsc -b`).

### Waiting

Nothing sleeps for a fixed period. Poll with `waitFor` or `expect.poll` against
the API, wait on the browser with `expect(locator)`, and use
`waitForSessionState` with an explicit budget for anything container-bound. The
one `waitForTimeout` in `tests/` is in `dropConnection`, where the sleep *is*
the outage being modelled.

The transcript is virtualised, so a row that is not in view is not in the DOM
and no locator can wait for it: read back with `reveal(page, locator)`, which
scrolls from the tail and waits for the virtualizer's re-render at each step.

### Adding a scenario

1. Put it in the spec file that owns the feature; add a spec file only for a
   feature none of them covers.
2. Arrange through the fixtures and the helpers, not through the UI: drive
   through the browser only what the scenario is actually about.
3. Add a row to the coverage table below, or extend an existing one, and run
   `node tests/coverage-check.mjs`.
4. If it needs a new `data-testid`, add the constant to `src/utils/testIds.ts`
   and re-export it from `tests/utils/test-ids.ts`. Prefer a role, a label or
   text.

## Coverage

One row per paragraph of `SPEC.md`, "User-facing features", and per sub-heading
of `SPEC.md`, "Frontend". `node tests/coverage-check.mjs` checks each
`` `<spec file>` › `<title>` `` reference below against the suite.

### `SPEC.md`, "User-facing features"

| Paragraph | Covered by |
| --- | --- |
| Login and invites | `auth.spec.ts` › `must change password before anything else`; `auth.spec.ts` › `login and logout`; `auth.spec.ts` › `login rejects a wrong password`; `auth.spec.ts` › `a deep link is preserved through login`; `auth.spec.ts` › `an admin invites a user who accepts via the logged link`; `auth.spec.ts` › `a revoked invitation cannot be accepted`; `auth.spec.ts` › `a password reset through the logged link replaces the password`; `auth.spec.ts` › `the settings page changes the password and keeps only this session`; the escalation opt-out is `settings.spec.ts` › `the escalation opt-out is saved, survives a reload and is what the API reports` |
| Projects | `projects.spec.ts` › `the new-project form refuses a remote that is not https`; `projects.spec.ts` › `a project created from a bare repository reaches ready without a reload`; `projects.spec.ts` › `a clone that fails shows its message and the retry succeeds`; `projects.spec.ts` › `fetch now moves the upstream ref and leaves the integration head`; `projects.spec.ts` › `a shared directory is added, refused twice, cleared and removed`; `projects.spec.ts` › `clearing and removing a shared directory are refused while a session runs`; `projects.spec.ts` › `the settings form renames the project and bounds max_attempts`; `projects.spec.ts` › `a project is deleted once its running session has ended` |
| Agent profiles | `projects.spec.ts` › `the default profile is edited and an ephemeral one is created beside it`; `projects.spec.ts` › `an unknown served state or tool is a 400 the editor cannot produce`; `session-view.spec.ts` › `ephemeral run once from the project page` |
| Sessions | `sessions.spec.ts` › `launch with a first message and watch the transcript`; `sessions.spec.ts` › `interject mid-turn`; `sessions.spec.ts` › `stop parks the session and shows stopped`; `sessions.spec.ts` › `sending a message to a parked session relaunches it`; `sessions.spec.ts` › `end moves to done and disables the composer`; `sessions.spec.ts` › `ending a session right after launch leaves no container`; `sessions.spec.ts` › `a CLI that exits non-zero fails the session and retry parks it`; `sessions.spec.ts` › `title defaults to the message's first line, truncated to 80 characters`; `sessions.spec.ts` › `an idle session is parked without user action`; `sessions.spec.ts` › `after the fixture is exhausted the stub echoes`; `session-view.spec.ts` › `terminal into a running container`; `session-view.spec.ts` › `metadata header` |
| Task board | `tasks.spec.ts` › `columns show the default states in order and an empty project invites a first task`; `tasks.spec.ts` › `a task created from the board form lands in backlog and opens in the drawer`; `tasks.spec.ts` › `an edit and a comment made in the drawer survive a reload`; `tasks.spec.ts` › `the drawer moves a card across columns and closes and reopens it`; `tasks.spec.ts` › `a blocks dependency blocks a card, clears when it closes, and refuses a cycle`; `tasks.spec.ts` › `a parent closes by itself when its last child closes`; `tasks.spec.ts` › `the states editor adds, renames and removes a column, and says why it cannot`; `tasks.spec.ts` › `release is disabled while no session holds the task`; `tasks.spec.ts` › `the drawer takes focus, keeps Tab inside it and gives focus back to the card`; `tasks.spec.ts` › `Escape asks before discarding a draft and shuts an open form first`; `task-sessions.spec.ts` › `open in session claims the task and the card shows its session`; `task-sessions.spec.ts` › `a held task cannot be opened in a second session`; `task-sessions.spec.ts` › `release from the drawer clears the claim without escalating`; `task-sessions.spec.ts` › `ending the session releases its task and says so on the thread`; `task-sessions.spec.ts` › `run once runs an ephemeral profile on the task and gives it back`; the escalation email is `task-sessions.spec.ts` › `an escalation at the attempt limit emails the assignee, and not one who opted out` |
| Git operations | `git.spec.ts` › `the session branch list shows ahead and behind`; `git.spec.ts` › `merging a session branch into main`; `git.spec.ts` › `rebasing the session branch onto main`; `git.spec.ts` › `pushing to the bare upstream, without a compare link for a file:// remote`; `git.spec.ts` › `a non-fast-forward push is refused until it is forced`; `git.spec.ts` › `a merge conflict lists the conflicting paths and leaves main alone`; `git.spec.ts` › `an upstream-tracking ref cannot be a mutation target`; `git.spec.ts` › `a rebase onto a dirty checkout asks the session to reconcile` |
| Secrets | `secrets.spec.ts` › `a global secret is created, replaced, renamed, flagged and deleted without ever showing its value`; `secrets.spec.ts` › `project secrets are shared and user secrets are the owner's or an admin's`; `secrets.spec.ts` › `a launch writes a use, and the uses view lists it without the value`; `secrets.spec.ts` › `a name that is not an environment-variable name is refused`; `secrets.spec.ts` › `an agent credential is added under its label, and a second kind at that scope is refused`; `secrets.spec.ts` › `the launch form warns without a credential, launches anyway, and names the credential once there is one` |
| Users (admin) | `admin.spec.ts` › `a member cannot open /admin`; `admin.spec.ts` › `an administrator sees the sections, promotes a member and demotes them again`; `admin.spec.ts` › `an administrator may step down but never delete itself`; `admin.spec.ts` › `a deleted user is signed out at its next request and cannot sign in again`; invites are the Login-and-invites row |
| Dashboard | `task-sessions.spec.ts` › `the dashboard lists running and parked sessions across projects`; `task-sessions.spec.ts` › `a task moved into needs_human shows on the dashboard` |

### `SPEC.md`, "Frontend"

| Sub-heading | Covered by |
| --- | --- |
| Agent credentials | `secrets.spec.ts` › `an agent credential is added under its label, and a second kind at that scope is refused`; the `AgentCredentialNotice` is `secrets.spec.ts` › `the launch form warns without a credential, launches anyway, and names the credential once there is one`, and its wording per scope is a Vitest test beside it (`src/secrets/AgentCredentialNotice.test.tsx`) |
| Copy links | `tasks.spec.ts` › `a task link opens the drawer directly and Copy link writes the canonical URL`; `tasks.spec.ts` › `a drawer opened by link falls back to the board, and a child link moves focus`; `session-view.spec.ts` › `copy link`; the return destination through login is `auth.spec.ts` › `a deep link is preserved through login` |
| Dashboard | `task-sessions.spec.ts` › `the dashboard lists running and parked sessions across projects`; `task-sessions.spec.ts` › `a task moved into needs_human shows on the dashboard` |
| Role templates | `projects.spec.ts` › `a profile started from the reviewer template is saved as reviewer-2`; the suffixing, the dropped states and the pre-fill itself are Vitest tests beside the helper (`src/pages/project/profileForm.test.ts`) |
| Session state | `session-view.spec.ts` › `full history after reload and in a second tab`; `session-view.spec.ts` › `reconnect resumes without duplicates`; `session-view.spec.ts` › `older history loads on scroll-up`; `session-reconnect.spec.ts` › `a reconnect keeps the session page mounted, terminal and all` |
| Transcript rendering | `sessions.spec.ts` › `second and third turns render subagent, edit diff, shell, deltas and denial`; `sessions.spec.ts` › `launch with a first message and watch the transcript` |
| Session side panel | `session-view.spec.ts` › `terminal into a running container`; `session-reconnect.spec.ts` › `a reconnect keeps the session page mounted, terminal and all`; the derived tab, the tab list's keys and the terminal never opening unasked are Vitest tests beside the component (`src/session/SidePanel.test.tsx`) |
| Tasks panel | `task-sessions.spec.ts` › `open in session claims the task and the card shows its session`; `task-sessions.spec.ts` › `run once runs an ephemeral profile on the task and gives it back`; the single listing, the fallback read and the invalidation path are Vitest tests beside the component (`src/session/TasksPanel.test.tsx`) |
| Changes panel | `git.spec.ts` › `the changes panel shows the session diff after a sync`; `git.spec.ts` › `the diff endpoint's own fetch-back emits no git event`; the compare link is an exclusion below |
| Task board | `tasks.spec.ts` › `columns show the default states in order and an empty project invites a first task`; `tasks.spec.ts` › `the drawer moves a card across columns and closes and reopens it`; `tasks.spec.ts` › `the states editor adds, renames and removes a column, and says why it cannot`; `task-sessions.spec.ts` › `open in session claims the task and the card shows its session`; the launch controls' disabled reasons are `task-sessions.spec.ts` › `a held task cannot be opened in a second session`; the drawer's state belonging to the task on display is `tasks.spec.ts` › `an edit open on one task is discarded when the drawer shows a cached other one` and `tasks.spec.ts` › `a draft in the drawer survives a refresh of the same task`; the drawer's focus, its modality and its Escape rules are `tasks.spec.ts` › `the drawer takes focus, keeps Tab inside it and gives focus back to the card`, `tasks.spec.ts` › `Escape asks before discarding a draft and shuts an open form first` and `tasks.spec.ts` › `a drawer opened by link falls back to the board, and a child link moves focus`, and the rule itself is a Vitest test beside it (`src/tasks/drawerEscape.test.ts`) |
| Task-board search | `tasks.spec.ts` › `board search matches titles and exact numbers, and resets on a project change` |
| Board refresh ordering | `tasks.spec.ts` › `a second browser context follows the first without reloading` |
| Hand-off controls | `handoffs.spec.ts` › `publishing a revision pins the commit and moves the task to review`; `handoffs.spec.ts` › `a reviewer's session starts from the hand-off commit and is told about it`; `handoffs.spec.ts` › `approving forwards the hand-off to merge and unlocks the task merge`; `handoffs.spec.ts` › `the task merge lands the pinned commit even after the branch advanced`; `handoffs.spec.ts` › `the merge control is shut without an approval and a superseded review is refused`; `handoffs.spec.ts` › `requesting changes sends the task back and a new revision resets the review`; `handoffs.spec.ts` › `the revision diff is read by hand-off id without syncing anything`; `handoffs.spec.ts` › `a commit that is not the source session's tip is refused`; `handoffs.spec.ts` › `a review of a superseded revision says the hand-off changed`; the base disclosure is `task-sessions.spec.ts` › `the launch form discloses the base the session will start from` |
| Composer | `sessions.spec.ts` › `interject mid-turn`; `sessions.spec.ts` › `end moves to done and disables the composer`; `session-view.spec.ts` › `ephemeral run once from the project page` |

The helper layer asserts itself in `helpers.spec.ts`, and `smoke.spec.ts` is the
one scenario that needs no stack.

### Exclusions

| What | Why, and where it is covered instead |
| --- | --- |
| Login throttling (429 after 10 failures in 15 minutes) | not covered in E2E: the limit is per client address, so tripping it would poison every later scenario in the run. Backend integration test (`orchestrator/tests/auth_login.rs`, `ten_failures_block_the_username_with_429`) |
| The last administrator is protected (409) | not covered in E2E: every spec file shares one database, so a demotion here is never the last one. Backend integration tests (`orchestrator/tests/users_last_admin_race.rs`, `orchestrator/tests/repositories_users.rs`) |
| A SIGTERM stop rendering as `killed` | not covered in E2E: the stub exits on SIGINT, so the escalation to SIGKILL is a timing the suite cannot force. Backend integration test (`orchestrator/tests/session_owner.rs`, `a_stop_escalates_to_sigterm_and_parks_the_session_as_killed`); the SIGINT half is `sessions.spec.ts` › `stop parks the session and shows stopped` |
| The GitHub compare link | skipped in E2E — `git.spec.ts` › `the github compare link is built client-side` — because the only remote the suite can push to is a local `file://` repository, which has no compare page. Its absence for a non-GitHub remote is asserted in `git.spec.ts` › `pushing to the bare upstream, without a compare link for a file:// remote`; the rule and the builder are `src/components/git/formState.test.ts` and `src/utils/github.test.ts` |
| A sync after a rebase with an unreconciled checkout | not covered in E2E: undecided, task `282ce` |

The seeded-administrator scenario — `auth.spec.ts` › `must change password before anything else` — is the suite's one conditional skip, and it skips only
on its *first* attempt: a retry that finds the account already used fails,
because that means the attempt being retried changed the password and then
failed. `node tests/coverage-check.mjs --skips` fails on any other skip in a
run.
