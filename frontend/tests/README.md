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

**Any number of runs can share one stack.** Two things accumulate on a stack
the suite has already run against, and the suite handles both:

- The seeded `admin`/`changeme` account is spent by the seeded-administrator
  scenario, so that scenario skips on every later run against the same stack —
  a documented skip (see the table below); `npm run test:e2e:up` starts from an
  empty database and brings the account back.
- The login throttle blocks the client address after 10 failed attempts in 15
  minutes (`SPEC.md`, "User-facing features" → "Login and invites"). A run
  makes three deliberate failures — a wrong password, a deleted user, and the
  replaced password after a reset — and a *re*run a fourth, because the probe
  that decides whether the seeded account is still fresh is a failed login too.
  Left alone, the third consecutive run would cross the limit and every login
  in it would be answered 429, so `tests/global-setup.ts`, Playwright's
  `globalSetup`, clears the throttle through `POST /api/test/throttle/reset`
  (`SPEC.md`, "Test-only routes") before every run, a single spec file or
  scenario included.

Nothing else in the suite cares: every other scenario makes its own user,
project and repository. One spec file:

```bash
npx playwright test tests/tasks.spec.ts
```

A single scenario, by title:

```bash
npx playwright test -g "interject mid-turn"
```

**Two projects, one stack.** `playwright.config.ts` runs every scenario in one
of two browsers, never both: `chromium` (Desktop Chrome) and `mobile` (a Pixel
7 — its viewport, touch and coarse pointer). The tag `@mobile` in a scenario's
title is what picks: `mobile` greps for it and `chromium` greps it out, so a
tagged scenario runs on the phone only and an untagged one on the desktop only.
A phone scenario is a scenario of its own, about what `SPEC.md`, "Frontend",
Mobile layout promises a phone, rather than a desktop scenario run twice. One
that branches on the browser asks `isMobile(testInfo)` from
`utils/test-helpers.ts`, and one that asserts nothing is wider than the screen
calls `expectNoHorizontalOverflow(page)` from there, the one spelling of that
check. The Pixel 7 is 412 px wide; a scenario about the narrowest phone the
layout is written for sets 360 × 740, with `test.use({ viewport })` or
`page.setViewportSize`, either of which keeps the device's touch and coarse
pointer. The phone scenarios alone:

```bash
npx playwright test --project mobile
```

The tag is part of the title, so the coverage table below quotes it too.

## Wall-clock

| Run | Machine | Time |
| --- | --- | --- |
| 104 scenarios, warm stack, `workers: 1` | Arch Linux developer machine, rootless Podman 6, 2026-09 | 5.9–8.7 minutes |

CI's `timeout-minutes: 45` covers the suite plus the stack build, with room to
spare.

The slowest scenario, and why:

| Scenario | Time | Bound by |
| --- | --- | --- |
| `sessions.spec.ts` › `interject mid-turn` | ~47 s | `MARS_STUB_LINE_DELAY_MS=200` pacing two fixture turns (58 and 159 lines) so the first one is still streaming while the interjection is typed |

Two scenarios used to sit beside it, bound by constants rather than by what
they assert, and no longer do:

| Scenario | Before | After | What changed |
| --- | --- | --- | --- |
| `sessions.spec.ts` › `an idle session is parked without user action` | 26–66 s | 5–11 s | it waited on the idle reaper's next tick, a hard-coded 60 s. The period is now `REAPER_INTERVAL_SECS` (`README.md`, "Configuration"), which `tests/e2e-stack.sh` sets to 2 |
| `session-view.spec.ts` › `older history loads on scroll-up` | 51–90 s | 16–54 s | the session has to commit more than one history page before a reload can leave any behind, and a page was a hard-coded 200 events. It is now a build-time constant, `VITE_SESSION_HISTORY_PAGE_SIZE`, which the dev server `playwright.config.ts` starts sets to 100, so the scenario sends about 36 inputs instead of about 69 |

Both columns were measured on the same machine in the same hour, each scenario
run twice alone and the "After" once more in a full run of the suite, under the
load of eight other agents building and testing on it (2026-09-26), so the
spread is the machine's and the figures are for comparing with each other, not
with the table above. Unloaded, the old figures were 33–46
s and ~27 s.

The page size stays the client's own choice rather than something the server
advertises: `GET /sessions/{id}/events` takes any `limit` up to 500, so no
contract changes with it. It is 100 and not smaller because the stub's three
recorded turns are 90 events: at 100 every other scenario still opens a session
with its whole history in the first page, as production does at 200, and only
this one, which fills past it on purpose, pages. The scenario reads the size
off the page's first history request instead of assuming it, so it holds
against a reused `npm run dev` or another origin that keeps the default.

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
| `api` | a REST client authenticated as `user`, whose user scope is given a fake agent credential so the launch forms take their ordinary path — a scope the dispatcher can never use ("Automation and the seeded roles" below). A second client — an admin, a reviewer — is `apiClient(request, token)` |
| `repo` | a local bare upstream with one commit on `main`, plus whatever `repoFiles` declares |
| `project` | a project cloned from `repo`, waited to `ready` |
| `sessions` | the scenario's sessions. `sessions.launch(...)` launches one; `sessions.track(client, id)` registers one the UI launched; `sessions.sweep(client, projectId)` registers a whole project, whose automation is paused and whose every session is ended at teardown. All three are cleaned up when the scenario finishes, so no container outlives a run |

Fixtures are lazy: a scenario that names none of them creates none, which is
why `smoke.spec.ts` › `an unauthenticated visit lands on the login route`
needs no stack at all.

`sessions.sweep` exists for one reason. The dispatcher and the scheduler launch
sessions nobody asked for (`SPEC.md`, "User-facing features" → "Automatic
dispatch" and "Scheduled agents"), so `dispatcher.spec.ts` and
`schedules.spec.ts` — the two specs that turn automation on — cannot know every
id they have to end, and one that arrives after the last assertion would
outlive the run. Every other spec registers each session as it launches it, and
needs none of this.

### Automation and the seeded roles

Every project a scenario creates is seeded with four profiles (`SPEC.md`, "Role
profile templates"; ADR 0051): `claude`, the default — conversational, serving
no state — which every launch that names no profile runs; `planner` over
`backlog`; and `implementer` over `ready` and `reviewer` over `review`, both
ephemeral and carrying `auto_launch`. The stack runs the dispatcher
(`DISPATCHER_INTERVAL_SECS=10`, woken by every task event), so those two would
claim any task a scenario puts in `ready` or `review` — except that the
dispatcher skips a profile whose agent credential does not resolve at `global`
or `project` scope (`ARCHITECTURE.md`, "Unattended launches" → "Eligibility").
The suite keeps it that way, and this is its one rule for it:

- **No scenario stores an agent credential at `global` scope.** One would make
  every seeded implementer and reviewer on the instance live at once, in every
  project of every spec that runs after it. The credential the `api` fixture
  gives each user is at `user` scope, which an unattended launch can never use,
  so the launch forms take their ordinary path and the dispatcher still has
  nothing to launch with.
- **A scenario that needs an unattended launch stores its credential at
  `project` scope, in its own project, and deletes it in `afterEach`** —
  `dispatcher.spec.ts` and `schedules.spec.ts`. Before storing it, it calls
  `turnOffAutoLaunch(api, project.id)`, which turns `auto_launch` off on every
  profile that carries it — on a new project, the seeded implementer and
  reviewer — so the only automation live in that project is the one the
  scenario turns on itself. Saving `auto_launch: false` needs no credential, so
  that order always works; the other order would leave a window in which the
  seeded roles are live. Such a scenario also registers its project with
  `sessions.sweep`, above.

So a scenario that moves tasks through `ready` and `review` by hand —
`task-sessions.spec.ts`, `handoffs.spec.ts`, `automerge.spec.ts` — needs no
arrangement against the dispatcher, and one that wants a profile with a
particular shape (a conversational one over `ready`, an ephemeral one to
`Run once`) creates it rather than relying on what was seeded. The seeded set
itself is asserted in one place, and the `claude` launch in another:

- `projects.spec.ts` › `a project created from a bare repository reaches ready without a reload`
- `sessions.spec.ts` › `a fresh project's launch form preselects claude and opens a conversation with a composer`

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
drops), `resources` (projects, profiles, secrets, sessions, tasks and their
hand-offs),
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
| Projects | `projects.spec.ts` › `the new-project form refuses a remote that is not https`; `projects.spec.ts` › `a project created from a bare repository reaches ready without a reload`; `projects.spec.ts` › `a clone that fails shows its message and the retry succeeds`; `projects.spec.ts` › `fetch now moves the upstream ref and leaves the integration head`; `projects.spec.ts` › `a shared directory is added, refused twice, cleared and removed`; `projects.spec.ts` › `clearing and removing a shared directory wait for the running session to end`; `projects.spec.ts` › `the settings form renames the project and bounds max_attempts`; `projects.spec.ts` › `a project is deleted once its running session has ended` |
| Agent profiles | the seeded four — `claude` the default, `implementer` and `reviewer` carrying `auto-launch` — are `projects.spec.ts` › `a project created from a bare repository reaches ready without a reload`; the launch form preselecting `claude` and opening a conversation with a composer is `sessions.spec.ts` › `a fresh project's launch form preselects claude and opens a conversation with a composer`; `projects.spec.ts` › `the default profile is edited and an ephemeral one is created beside it`; `projects.spec.ts` › `an unknown served state or tool is a 400 the editor cannot produce`; `session-view.spec.ts` › `ephemeral run once from the project page` |
| Sessions | `sessions.spec.ts` › `launch with a first message and watch the transcript`; `sessions.spec.ts` › `interject mid-turn`; `sessions.spec.ts` › `stop parks the session and shows stopped`; `sessions.spec.ts` › `sending a message to a parked session relaunches it`; `sessions.spec.ts` › `end moves to done and disables the composer`; `sessions.spec.ts` › `ending a session right after launch leaves no container`; `sessions.spec.ts` › `a CLI that exits non-zero fails the session and retry parks it`; `sessions.spec.ts` › `title defaults to the message's first line, truncated to 80 characters`; `sessions.spec.ts` › `an idle session is parked without user action`; `sessions.spec.ts` › `after the fixture is exhausted the stub echoes`; `session-view.spec.ts` › `terminal into a running container`; `session-view.spec.ts` › `metadata header`; a failed session's reason as text on its row of the project's sessions tab, on a phone with no tooltip, is `task-sessions.spec.ts` › `a failed session's reason is visible on a phone without a tooltip @mobile`, with its two-line clamp and `Show all` a Vitest test (`src/pages/project/SessionsTab.test.tsx`) |
| Task board | `tasks.spec.ts` › `columns show the default states in order and an empty project invites a first task`; `tasks.spec.ts` › `a task created from the board form lands in backlog and opens in the drawer`; `tasks.spec.ts` › `an edit and a comment made in the drawer survive a reload`; `tasks.spec.ts` › `the drawer moves a card across columns and closes and reopens it`; `tasks.spec.ts` › `a blocks dependency blocks a card, clears when it closes, and refuses a cycle`; `tasks.spec.ts` › `a parent closes by itself when its last child closes`; `tasks.spec.ts` › `the states editor adds, renames and removes a column, and says why it cannot`; `tasks.spec.ts` › `release is disabled while no session holds the task`; `tasks.spec.ts` › `the drawer takes focus, keeps Tab inside it and gives focus back to the card`; `tasks.spec.ts` › `Escape asks before discarding a draft and shuts an open form first`; `tasks.spec.ts` › `Escape shuts a hand-off form and a confirmation before the drawer`; `task-sessions.spec.ts` › `open in session claims the task and the card shows its session`; `task-sessions.spec.ts` › `a held task cannot be opened in a second session`; `task-sessions.spec.ts` › `release from the drawer clears the claim without escalating`; `task-sessions.spec.ts` › `ending the session releases its task and says so on the thread`; `task-sessions.spec.ts` › `run once runs an ephemeral profile on the task and gives it back`; the escalation email is `task-sessions.spec.ts` › `an escalation at the attempt limit emails the assignee, and not one who opted out` |
| Automatic dispatch | `dispatcher.spec.ts` › `the dispatcher picks up a task moved into a served state, and a pause stops the next one` |
| Automatic merge | `automerge.spec.ts` › `an approved hand-off in merge is merged without a session and closes the task`; `automerge.spec.ts` › `a conflicting hand-off comes back to ready with its paths and main is unchanged`; `automerge.spec.ts` › `turning auto-merge off in the states editor leaves the next approved task in merge` — the seeded `merge` column's `auto-merge` chip, the `Merged <commit> into main.` comment and the moved `main`, the conflict comment's paths and the untouched `main`, and the editor's toggle with a second auto-merge state as the barrier that proves the job ran; the manual merge that `handoffs.spec.ts` drives turns the flag off first |
| Round limit | `automerge.spec.ts` › `a conflicting merge at the round limit escalates to needs_human with the reason` — `Max rounds` set in the settings form, the card's `round 2/2`, and the system's conflict send-back at the limit landing in `needs_human` with `round limit reached (2/2): …`; a session's `changes_requested` forward at the limit is not driven here, because the stub image calls no MCP tool, and is a backend integration test (`orchestrator/tests/tracker_rounds.rs`, `a_session_send_back_at_the_limit_goes_to_the_human_state`) |
| Scheduled agents | `schedules.spec.ts` › `a due schedule launches a session nobody asked for, and a pause stops the next tick` — the schedule set in the editor, the next run the server computed, the tick made due through `POST /api/test/scheduler-tick`, the session that appears and runs to `done`, and the paused project refusing the next tick |
| Git operations | `git.spec.ts` › `the session branch list shows ahead and behind`; `git.spec.ts` › `merging a session branch into main`; `git.spec.ts` › `rebasing the session branch onto main`; `git.spec.ts` › `pushing to the bare upstream, without a compare link for a file:// remote`; `git.spec.ts` › `a non-fast-forward push is refused until it is forced`; `git.spec.ts` › `a merge conflict lists the conflicting paths and leaves main alone`; `git.spec.ts` › `a conflicting merge launches a resolver session from its target`; `git.spec.ts` › `an upstream-tracking ref cannot be a mutation target`; `git.spec.ts` › `a rebase onto a dirty checkout asks the session to reconcile` |
| Secrets | `secrets.spec.ts` › `a global secret is created, replaced, renamed, flagged and deleted without ever showing its value`; `secrets.spec.ts` › `project secrets are shared and user secrets are the owner's or an admin's`; `secrets.spec.ts` › `a launch writes a use, and the uses view lists it without the value`; `secrets.spec.ts` › `a name that is not an environment-variable name is refused`; `secrets.spec.ts` › `an agent credential is added under its label, and a second kind at that scope is refused`; `secrets.spec.ts` › `the launch form warns without a credential, launches anyway, and names the credential once there is one` |
| Users (admin) | `admin.spec.ts` › `a member cannot open /admin`; `admin.spec.ts` › `an administrator sees the sections, promotes a member and demotes them again`; `admin.spec.ts` › `an administrator may step down but never delete itself`; `admin.spec.ts` › `a deleted user is signed out at its next request and cannot sign in again`; invites are the Login-and-invites row |
| Dashboard | `task-sessions.spec.ts` › `the dashboard lists running and parked sessions across projects`; `task-sessions.spec.ts` › `a task moved into needs_human shows on the dashboard` |

### `SPEC.md`, "Frontend"

| Sub-heading | Covered by |
| --- | --- |
| Agent credentials | `secrets.spec.ts` › `an agent credential is added under its label, and a second kind at that scope is refused`; the `AgentCredentialNotice` is `secrets.spec.ts` › `the launch form warns without a credential, launches anyway, and names the credential once there is one`, and its wording per scope is a Vitest test beside it (`src/secrets/AgentCredentialNotice.test.tsx`) |
| Confirmations | `projects.spec.ts` › `a shared directory is added, refused twice, cleared and removed`; `projects.spec.ts` › `a project is deleted once its running session has ended`; `projects.spec.ts` › `the default profile is edited and an ephemeral one is created beside it`; `admin.spec.ts` › `an administrator may step down but never delete itself`; `admin.spec.ts` › `a deleted user is signed out at its next request and cannot sign in again`; `auth.spec.ts` › `a revoked invitation cannot be accepted`; `secrets.spec.ts` › `a global secret is created, replaced, renamed, flagged and deleted without ever showing its value`; `tasks.spec.ts` › `the states editor adds, renames and removes a column, and says why it cannot`; `sessions.spec.ts` › `end moves to done and disables the composer`; `sessions.spec.ts` › `ending and deleting a session warn of its unmerged commits, and the delete takes its branch`; the drawer's confirmations are the Task board row, and the panel's own pending, refusal and tones are Vitest tests beside the callers |
| Help | `help.spec.ts` › `a field's Learn more link opens its help topic at the anchored section`; `help.spec.ts` › `the help page's contents and topic links move between anchored sections`; `help.spec.ts` › `an auto-merge state's conflict state links the task-flow topic` — the seeded `merge` state's `Conflict state` hint as its description and its link; `helpPath`, topic parsing, the `help:` link resolution and `FieldShell`'s `help` leaving `aria-describedby` on the hint alone are Vitest tests beside them (`src/help/topics.test.ts`, `src/help/content.test.ts`, `src/components/FieldShell.test.tsx`) |
| Project page | `git.spec.ts` › `the branches tab holds the git panel and the sessions tab does not` — the Sessions tab without the git panel, the `Branches` tab link, and the tab's integration heads, `History`, `Merge any ref` and session-branch table in that order; every other `git.spec.ts` scenario on the project page opens `?tab=branches`; the tab list's order and `parseProjectTab` are Vitest tests beside the registry (`src/pages/project/tabs.test.ts`), and the heads' order and upstream pairing beside their helper (`src/components/git/integrationHeads.test.ts`); a head's `Push…` is `git.spec.ts` › `pushing main from its integration head row` — the form under the head's row with the remote branch defaulting to `main`, the rejection while upstream has advanced, and the push that lands once `origin/main` is merged in, with no compare link; the head's `History` and `Revert to here` are `git.spec.ts` › `reverting main to before two task merges reopens the tasks without their hand-offs` — two tasks auto-merged through the API, their rows attributed to them, the confirmation's list, the reopen into `ready` with a comment, the success note, the revert commit on top of the history and highlighted, the two rows it undid marked `undone by` it, the base row labelled `current content` without a second `Revert to here`, `main`'s tree back at the base and both tasks in `ready` with no current hand-off; the rows a revert undoes, the tasks it names, how a row is marked (head, undone, current content, revertible), the `Requested-By` reading and the page cursor are Vitest tests beside their helper (`src/components/git/history.test.ts`), and what the confirmation sends, its empty-comment refusal and the `branch has moved` and `nothing to revert` advice are beside the panel (`src/components/git/RevertConfirm.test.tsx`) |
| Resolve with an agent (Project page) | `git.spec.ts` › `a conflicting merge launches a resolver session from its target` — two sessions rewriting the same file, the first merged through the API, the second's merge from its row listing the path, `Resolve with an agent` opening the form with the default profile preselected and the generated message naming the source ref, its commit and the path, and the launch landing on a session whose base is `main`, with no task, and whose first user message is that text; the message for each source kind and without a commit, and the profile preselection (`resolver`, else the default, conversational only), are Vitest tests beside the helper (`src/components/git/resolveMessage.test.ts`) |
| Copy links | `tasks.spec.ts` › `a task link opens the drawer directly and Copy link writes the canonical URL`; `tasks.spec.ts` › `a drawer opened by link falls back to the board, and a child link moves focus`; `session-view.spec.ts` › `copy link`; the return destination through login is `auth.spec.ts` › `a deep link is preserved through login` |
| Dashboard | `task-sessions.spec.ts` › `the dashboard lists running and parked sessions across projects`; `task-sessions.spec.ts` › `a task moved into needs_human shows on the dashboard` |
| Role templates | `projects.spec.ts` › `a profile started from the reviewer template is saved as reviewer-2` — including the `auto_launch` the template brings, refused at the toggle while the project has no credential an unattended launch could use, and saved once it is unticked; the suffixing, the dropped states and the pre-fill itself are Vitest tests beside the helper (`src/pages/project/profileForm.test.ts`) |
| Unattended launches | `dispatcher.spec.ts` › `the dispatcher picks up a task moved into a served state, and a pause stops the next one` — with the seeded roles' `auto_launch` turned off first ("Automation and the seeded roles" above), the profile editor's toggle, the project form's `Pause automation` and the header's `automation paused` chip; the credential warning under the toggle and the `max_concurrent` floor are Vitest tests beside the helpers (`src/pages/project/profileForm.test.ts`, `src/pages/project/projectSettings.test.ts`) |
| Scheduled profiles | `schedules.spec.ts` › `a due schedule launches a session nobody asked for, and a pause stops the next tick` — the `Schedule` fieldset's two fields, the `schedule` chip in the profiles list and the read-only `Next run`/`Last run` outputs; the field-by-field routing of the server's refusals and the placeholder are Vitest tests beside the editor (`src/pages/project/ProfilesTab.test.tsx`) |
| Launch source | `dispatcher.spec.ts` › `the dispatcher picks up a task moved into a served state, and a pause stops the next one` — the `dispatcher` tag in the project's session list and in the session header; `schedules.spec.ts` › `a due schedule launches a session nobody asked for, and a pause stops the next tick` — the `schedule` tag in both places; a person's session carrying no tag is every other scenario in the suite |
| Session state | `session-view.spec.ts` › `full history after reload and in a second tab`; `session-view.spec.ts` › `reconnect resumes without duplicates`; `session-view.spec.ts` › `older history loads on scroll-up`; `session-reconnect.spec.ts` › `a reconnect keeps the session page mounted, terminal and all` |
| Transcript rendering | `sessions.spec.ts` › `second and third turns render subagent, edit diff, shell, deltas and denial`; `sessions.spec.ts` › `launch with a first message and watch the transcript`; `sessions.spec.ts` › `an edit diff is unified on a phone and the transcript scrolls as one @mobile` — the edit diff without its side-by-side toggle, neither it nor the subagent's report inside a vertical scroller of its own, and the page no wider than the screen, while the toggle's class and the choice kept across `sm` are a Vitest test beside the component (`src/components/DiffView.test.tsx`) |
| Session side panel | `session-view.spec.ts` › `terminal into a running container`; `session-reconnect.spec.ts` › `a reconnect keeps the session page mounted, terminal and all`; `session-view.spec.ts` › `the side panel opens as a sheet on a phone and the transcript keeps the width @mobile`; the derived tab, the tab list's keys, the terminal never opening unasked and the sheet below `lg` (dialog, focus, Escape, overlay, closed by a crossing of `lg`) are Vitest tests beside the component (`src/session/SidePanel.test.tsx`) |
| Tasks panel | `task-sessions.spec.ts` › `open in session claims the task and the card shows its session`; `task-sessions.spec.ts` › `run once runs an ephemeral profile on the task and gives it back`; the single listing, the fallback read and the invalidation path are Vitest tests beside the component (`src/session/TasksPanel.test.tsx`) |
| Changes panel | `git.spec.ts` › `the changes panel shows the session diff after a sync`; `git.spec.ts` › `the diff endpoint's own fetch-back emits no git event`; an ended session that kept no ref is `sessions.spec.ts` › `end moves to done and disables the composer` (no commits: no changes) and `handoffs.spec.ts` › `an implementer that handed off its tip ends with no ref and still shows its changes` (its work, and the no-branch note), whose rule per branch listing is a Vitest test beside the component (`src/session/ChangesPanel.test.tsx`); the compare link is an exclusion below |
| Task board | `tasks.spec.ts` › `columns show the default states in order and an empty project invites a first task`; `tasks.spec.ts` › `the drawer moves a card across columns and closes and reopens it`; `tasks.spec.ts` › `the states editor adds, renames and removes a column, and says why it cannot`; `task-sessions.spec.ts` › `open in session claims the task and the card shows its session`; the launch controls' disabled reasons are `task-sessions.spec.ts` › `a held task cannot be opened in a second session`; the drawer's state belonging to the task on display is `tasks.spec.ts` › `an edit open on one task is discarded when the drawer shows a cached other one` and `tasks.spec.ts` › `a draft in the drawer survives a refresh of the same task`; the drawer's focus, its modality and its Escape rules are `tasks.spec.ts` › `the drawer takes focus, keeps Tab inside it and gives focus back to the card`, `tasks.spec.ts` › `Escape asks before discarding a draft and shuts an open form first`, `tasks.spec.ts` › `Escape shuts a hand-off form and a confirmation before the drawer` and `tasks.spec.ts` › `a drawer opened by link falls back to the board, and a child link moves focus`, the phone's board — one column per swipe, the chip row that picks one, the drawer header on one row — and a tapped `Close` asking about a draft are `tasks.spec.ts` › `a phone swipes the board one column at a time and a tapped Close keeps a draft @mobile`, with the chip row's marking and scrolling a Vitest test beside the board (`src/tasks/TaskBoard.test.tsx`), and the rule itself — including which of the drawer's sub-forms a press belongs to — is a Vitest test beside it (`src/tasks/drawerEscape.test.ts`); what one launch settles, what it sends without a chosen base, and the toggles and `Cancel` it shuts while it runs are Vitest tests beside the two forms (`src/tasks/LaunchForTask.test.tsx`, `src/pages/project/LaunchSessionForm.test.tsx`), including the launch whose form is gone before the answer and therefore navigates nowhere; the states editor's `Auto-merge` toggle and `Conflict state` select and the column's `auto-merge` chip are `automerge.spec.ts` › `turning auto-merge off in the states editor leaves the next approved task in merge` and `automerge.spec.ts` › `an approved hand-off in merge is merged without a session and closes the task`, and the card's `round n/max` label with the settings form's `Max rounds` is `automerge.spec.ts` › `a conflicting merge at the round limit escalates to needs_human with the reason`; each card chip's accessible name, its meaning, is a Vitest test (`src/tasks/TaskCard.test.tsx`); the line saying a task waits for its author session's branch, on the card and in the drawer, and its going once the branch is merged, is `git.spec.ts` › `a task filed by a session waits on the board until its branch reaches main` — the task filed over MCP with the session's own token (`tests/utils/mcp.ts`), since the stub calls no tool — and the join itself (ahead, no ref, no author, closed, held) is a Vitest test beside it (`src/tasks/authorBranch.test.ts`) |
| Task-board search | `tasks.spec.ts` › `board search matches titles and exact numbers, and resets on a project change` |
| Board refresh ordering | `tasks.spec.ts` › `a second browser context follows the first without reloading` |
| Hand-off controls | `handoffs.spec.ts` › `publishing a revision pins the commit and moves the task to review`; `handoffs.spec.ts` › `a reviewer's session starts from the hand-off commit and is told about it`; `handoffs.spec.ts` › `approving forwards the hand-off to merge and unlocks the task merge`; `handoffs.spec.ts` › `the task merge lands the pinned commit even after the branch advanced`; `handoffs.spec.ts` › `the merge control is shut without an approval and a superseded review is refused`; `handoffs.spec.ts` › `requesting changes sends the task back and a new revision resets the review`; `handoffs.spec.ts` › `the revision diff is read by hand-off id without syncing anything`; `handoffs.spec.ts` › `a commit that is not the source session's tip is refused`; `handoffs.spec.ts` › `a review of a superseded revision says the hand-off changed`; dropping the current hand-off, its required comment and the history that keeps it are `handoffs.spec.ts` › `dropping the hand-off clears it from the drawer and keeps it in the history`; the base disclosure is `task-sessions.spec.ts` › `the launch form discloses the base the session will start from` |
| Mobile layout | Every `@mobile` scenario of the suite, each asserting that nothing is wider than the screen through the one helper `expectNoHorizontalOverflow(page)` (`utils/layout.ts`). The gate is `smoke.spec.ts` › `every route fits a phone without horizontal scroll @mobile` — at 360 × 740, the narrowest phone the layout is written for, the four auth pages signed out, then an administrator with a project holding one session, one task, two secrets, one shared directory and a scheduled profile, on `/`, `/projects`, every tab of `PROJECT_TABS`, the task drawer, the session folded, unfolded and with the side panel's sheet on each tab (the terminal attached to the running stub container), `/secrets`, `/settings`, `/help` and `/admin`, where no document is wider than the viewport, the browser has not zoomed out to fit one, and no box in `main` reaches past the right edge outside a scroller of its own, the failure listing each offender. The rest, one promise each: the first paint is `smoke.spec.ts` › `the console loads on a phone @mobile` — login and the dashboard; the phone's navigation is `smoke.spec.ts` › `the top nav and project tabs fit a phone @mobile` — on the Pixel 7's 412 px, an administrator's five icon-only nav links, each at least 44 px wide, with Help and Log out inside the viewport and neither the nav nor the header row scrolling sideways, and every project tab inside the viewport with the tab strip wrapping rather than scrolling, while every nav link keeping its label as its accessible name is a Vitest test (`src/components/PageLayout.test.tsx`); the pointer rule is `settings.spec.ts` › `the password form is usable on a phone @mobile` — a focused field computes to 16 px and every visible button is at least 44 px tall, and the constants that carry it are guarded by a Vitest test (`src/components/fieldStyles.test.ts`); the tables are `projects.spec.ts` › `the projects and sessions tables fit a phone @mobile` — a project with a remote URL far wider than a 360 px screen, and a session in it, leave `/projects` and the project's sessions tab no wider than the viewport — while the scrollers' positioning is a Vitest test (`src/components/TableHead.test.tsx`) and the fields that ride under a row's name are Vitest tests beside their tables (`src/pages/AdminPage.test.tsx`, `src/tasks/TaskStatesEditor.test.tsx`); a failed session's reason is `task-sessions.spec.ts` › `a failed session's reason is visible on a phone without a tooltip @mobile`, and a shut `Auto-merge` toggle's reason a Vitest test (`src/tasks/TaskStatesEditor.test.tsx`); the reactive width behind the side panel's open-or-rail is a Vitest test beside the hook (`src/hooks/useMediaQuery.test.ts`) and its resize across `lg` beside the panel (`src/session/SidePanel.test.tsx`); the side panel as a sheet is `session-view.spec.ts` › `the side panel opens as a sheet on a phone and the transcript keeps the width @mobile`; the folded session header is `session-view.spec.ts` › `the session header folds on a phone and the transcript keeps its height @mobile` — the metadata and the actions hidden until `Details` is tapped, the transcript at least 40 % of the viewport's height with `Details` open, and the branch section as a sheet — while the disclosure, the whole ids, the branch sheet's focus and the title's cancel button are Vitest tests beside the header (`src/session/SessionHeader.test.tsx`), the visual viewport's height beside its hook (`src/hooks/useVisualViewportHeight.test.ts`) and the re-pin on a resize of the transcript beside `useStickToBottom` (`src/session/useStickToBottom.test.tsx`); the composer is `sessions.spec.ts` › `a phone types a two-line message and sends it with the button @mobile`; the unified diff and the one transcript scroller are `sessions.spec.ts` › `an edit diff is unified on a phone and the transcript scrolls as one @mobile`; and the board and its drawer are `tasks.spec.ts` › `a phone swipes the board one column at a time and a tapped Close keeps a draft @mobile` |
| Composer | `sessions.spec.ts` › `interject mid-turn`; `sessions.spec.ts` › `a phone types a two-line message and sends it with the button @mobile` — on a coarse pointer Enter is a newline and the button sends, the message renders two lines, and the pointer and width rules beside it are Vitest (`src/session/Composer.test.tsx`); `sessions.spec.ts` › `a fresh project's launch form preselects claude and opens a conversation with a composer`; `sessions.spec.ts` › `end moves to done and disables the composer`; `session-view.spec.ts` › `ephemeral run once from the project page` |

The helper layer asserts itself in `helpers.spec.ts`, and `smoke.spec.ts` ›
`an unauthenticated visit lands on the login route` is the one scenario that
needs no stack.

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
