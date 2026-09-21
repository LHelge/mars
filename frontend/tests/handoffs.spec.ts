// "Code hand-offs and review" from the user's side (`SPEC.md`, "Code hand-offs
// and review"; "Git": the task form of `MergeInput` and `diff?handoff_id=`;
// "Sessions": the hand-off default base and the generated first message;
// "Frontend", "Hand-off controls"; `ARCHITECTURE.md`, "Git model": the
// `refs/handoffs/<id>` retention; ADR 0018).
//
// One sentence per scenario: publishing a revision moves the task and pins the
// commit, a reviewer's session starts from that commit and is told about it,
// approving forwards the hand-off and unlocks the task merge, the merge lands
// the pinned commit and not the branch tip, an unapproved or superseded
// hand-off is refused, requesting changes sends the task back without losing
// the commit, the revision diff is read by hand-off id without syncing, and a
// commit that is not the source session's tip is refused.
//
// **Where the work comes from.** The stub CLI replays a transcript and executes
// nothing, so — exactly as `git.spec.ts` does — a scenario stands in for the
// agent and commits into `<DATA_DIR>/sessions/<sid>/work` from the host. The
// hand-off's own publication is what syncs that clone into the mirror: it does
// so *silently*, recording no `git` event (`ARCHITECTURE.md`, "MCP design",
// Side effects), which is what `the revision diff is read by hand-off id`
// relies on.
//
// **Sessions are launched without a first message**, so the stub blocks on
// stdin and the transcript holds nothing but what the orchestrator generated.
//
// **Three places where the task's acceptance criteria and the landed
// application disagree**, each resolved in favour of the application:
//
//   - A hand-off "without a state change" cannot be expressed in the form at
//     all: `RevisionForm` and `ReviewForm` both drop the task's current state
//     from their `Move to` select, and submit is disabled until a state and a
//     comment are there. Both refusals are asserted as the forms make them —
//     the missing option and the dead button — and the API's own 400 is
//     asserted beside them, because that is the contract the form is standing
//     in front of.
//   - The stale-`handoff_id` 409 is asserted through REST. The UI cannot send
//     one: `ReviewForm` forwards `task.handoff.id`, and the drawer refetches
//     the task on every task event, so by the time a superseding revision
//     exists the open form is already forwarding the new id. What the UI owes
//     is the opposite guarantee — that its approve always uses the current id —
//     and that is what is asserted after the refusal.
//   - The second revision of `merge is disabled without approval` and the `D`
//     of `changes requested` are published from the same implementer session
//     rather than a fresh one. What resets the review status is the revision,
//     not the session it came from, and a second container per scenario buys
//     the assertion nothing.

import type {
  APIRequestContext,
  Browser,
  Locator,
  Page,
} from "@playwright/test";

import type {
  Handoff,
  Project,
  Session,
  SyncResult,
  TaskDetail,
} from "../src/types";
import type { Api } from "./utils/test-helpers";
import { apiClient, expect, test } from "./utils/fixtures";
import type { SessionTracker } from "./utils/fixtures";
import {
  commitInSessionWorkClone,
  createTask,
  endSession,
  createTestUser,
  getTask,
  gitIsAncestor,
  gitRevParse,
  loginViaToken,
  mirrorPath,
  newLoggedInPage,
  reveal,
  seedAgentCredential,
  transcript,
  waitFor,
  waitForSessionState,
  type TestUser,
} from "./utils/test-helpers";

// The upstream every scenario here clones: the file the commits rewrite.
test.use({ repoFiles: { "src/app.txt": "v1\n" } });

// A container start, a clone, a hand-off publication that syncs, and — in the
// reviewer scenario — three sessions in a row.
test.setTimeout(240_000);

/** A live board refresh is coalesced, so nothing here asserts immediacy. */
const LIVE_TIMEOUT = 15_000;

interface Stage {
  client: Api;
  project: Project;
  sessions: SessionTracker;
  /** `<DATA_DIR>/projects/<pid>/repo.git`, where every ref assertion is made. */
  mirror: string;
}

/**
 * The implementer U1's arrangement on top of the shared fixtures: one task
 * `Add greeting` in `ready`, on the project the `project` fixture cloned.
 */
async function stage(
  sessions: SessionTracker,
  client: Api,
  project: Project,
): Promise<Stage> {
  await createTask(client, project.id, {
    title: "Add greeting",
    state: "ready",
  });
  return { client, project, sessions, mirror: mirrorPath(project.id) };
}

/**
 * A session launched for the task over REST and waited to `running`, which is
 * what says its work clone exists. Launching from the drawer is
 * `task-sessions.spec.ts`; here it is arrangement.
 */
async function implementer(stage: Stage): Promise<string> {
  const session = await stage.sessions.launch(stage.client, stage.project.id, {
    task_id: 1,
  });
  await waitForSessionState(stage.client, session.id, "running", 120_000);
  return session.id;
}

/** `POST /sessions/{id}/sync` (`SPEC.md`, "Sessions"): the explicit fetch-back. */
function syncSession(client: Api, sessionId: string): Promise<SyncResult> {
  return client.post<SyncResult>(`/sessions/${sessionId}/sync`);
}

/** The REST revision publication the UI scenarios arrange with. */
async function publishRevision(
  stage: Stage,
  source: string,
  commit: string,
  comment: string,
  state: string,
): Promise<Handoff> {
  await stage.client.put(`/projects/${stage.project.id}/tasks/1`, {
    state,
    handoff: {
      kind: "revision",
      source_session_id: source,
      commit,
      comment,
    },
  });
  return currentHandoff(stage);
}

/** The REST forward, for the scenarios whose subject is something else. */
async function forwardHandoff(
  stage: Stage,
  client: Api,
  handoffId: string,
  comment: string,
  state: string,
  review?: "approved" | "changes_requested",
): Promise<Handoff> {
  await client.put(`/projects/${stage.project.id}/tasks/1`, {
    state,
    handoff: {
      kind: "forward",
      handoff_id: handoffId,
      comment,
      ...(review === undefined ? {} : { review }),
    },
  });
  return currentHandoff(stage);
}

/**
 * Waits until the task is in `state` and returns it. A hand-off the browser
 * published settles in its own time; the panel's own text is not a reliable
 * gate, because the history above it already carries the words a fresh
 * revision will show.
 */
function waitForTaskState(stage: Stage, state: string): Promise<TaskDetail> {
  return waitFor(
    async () => {
      const task = await getTask(stage.client, stage.project.id, 1);
      return task.state === state ? task : null;
    },
    { timeoutMs: LIVE_TIMEOUT, description: `task #1 to reach ${state}` },
  );
}

/** The task's current hand-off, which every scenario asserts something about. */
async function currentHandoff(stage: Stage): Promise<Handoff> {
  const task = await getTask(stage.client, stage.project.id, 1);
  if (task.handoff === null) {
    throw new Error(`task #1 of ${stage.project.id} has no current hand-off`);
  }
  return task.handoff;
}

// --- the drawer -------------------------------------------------------------

/** Opens the task drawer by its own route, which is the board with it open. */
async function openTask(page: Page, project: Project): Promise<Locator> {
  await page.goto(`/projects/${project.id}/tasks/1`);
  const panel = page.getByRole("dialog");
  await expect(panel).toHaveAttribute("aria-label", "Task #1");
  return panel;
}

/** The drawer's `Code hand-off` section. */
function handoffSection(panel: Locator): Locator {
  return panel
    .getByRole("heading", { name: "Code hand-off" })
    .locator("xpath=ancestor::section[1]");
}

/** Opens the revision form and returns it. */
async function openRevisionForm(panel: Locator): Promise<Locator> {
  await handoffSection(panel)
    .getByRole("button", { name: "Publish revision", exact: true })
    .first()
    .click();
  const form = panel.getByRole("form", { name: "Publish revision" });
  await expect(form).toBeVisible();
  return form;
}

/** Fills and submits the revision form. */
async function submitRevision(
  form: Locator,
  fields: { source: string; commit: string; comment: string; state: string },
): Promise<void> {
  await form.getByLabel("Source session").selectOption(fields.source);
  await form.getByLabel("Commit", { exact: true }).fill(fields.commit);
  await form.getByLabel("Comment", { exact: true }).fill(fields.comment);
  await form.getByLabel("Move to").selectOption(fields.state);
  await form.getByRole("button", { name: "Publish revision" }).click();
}

type Decision = "Approve" | "Request changes" | "Forward without decision";

/** Opens one of the three review forms and returns it. */
async function openReviewForm(
  panel: Locator,
  decision: Decision,
): Promise<Locator> {
  await handoffSection(panel)
    .getByRole("button", { name: decision, exact: true })
    .first()
    .click();
  const form = panel.getByRole("form", { name: decision });
  await expect(form).toBeVisible();
  return form;
}

/** Fills and submits an open review form. */
async function submitReview(
  form: Locator,
  decision: Decision,
  state: string,
  comment: string,
): Promise<void> {
  await form.getByLabel("Move to").selectOption(state);
  await form.getByLabel("Comment", { exact: true }).fill(comment);
  await form.getByRole("button", { name: decision, exact: true }).click();
}

/** The drawer's merge control, which only an approved hand-off enables. */
function mergeButton(panel: Locator): Locator {
  return handoffSection(panel)
    .getByRole("button", { name: "Merge approved hand-off" })
    .first();
}

/** The task drawer's launch form, opened on its `Open in session` button. */
async function openLaunchForm(panel: Locator): Promise<Locator> {
  await panel.getByRole("button", { name: "Open in session", exact: true }).click();
  const form = panel.getByRole("form", { name: "Open in session" });
  await expect(form).toBeVisible();
  return form;
}

/** Submits a launch form and returns the session the app navigated to. */
async function submitLaunch(
  page: Page,
  form: Locator,
  tracker: SessionTracker,
  client: Api,
): Promise<string> {
  await form.getByRole("button", { name: "Open in session", exact: true }).click();
  await page.waitForURL(/\/sessions\/[0-9a-f-]{8}-/);
  const id = page.url().slice(page.url().lastIndexOf("/") + 1);
  return tracker.track(client, id);
}

// --- the session view -------------------------------------------------------

/** A second browser signed in as the reviewer U2. */
async function reviewer(
  browser: Browser,
  request: APIRequestContext,
): Promise<{ user: TestUser; client: Api; page: Page }> {
  const user = await createTestUser(request, { prefix: "reviewer" });
  const client = apiClient(request, user.access_token);
  // A second user is not the `api` fixture's, so it needs its own credential:
  // this one launches from the drawer, whose button would otherwise read
  // `Launch anyway` (`SPEC.md`, "Frontend", Agent credentials).
  await seedAgentCredential(client);
  return {
    user,
    client,
    page: await newLoggedInPage(browser, user),
  };
}

// --- scenarios --------------------------------------------------------------

test("publishing a revision pins the commit and moves the task to review", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const fixture = await stage(sessions, api, project);
  const session = await implementer(fixture);
  const commit = commitInSessionWorkClone(
    session,
    { "greeting.txt": "hello\n" },
    "feat: greeting",
  );

  const panel = await openTask(page, project);
  await expect(handoffSection(panel).getByText("No code hand-off")).toBeVisible();

  const form = await openRevisionForm(panel);
  // Nothing is preselected but the one session that could be meant
  // (`tasks/handoffRules.ts`: a single usable candidate is a choice with no
  // alternative), and the state the task is already in is not on offer at all.
  await expect(form.getByLabel("Source session")).toHaveValue(session);
  await expect(
    form.getByLabel("Move to").locator("option", { hasText: "ready" }),
  ).toHaveCount(0);
  // An empty comment cannot be sent: the API requires one, and the form makes
  // that visible before the request (`SPEC.md`: `comment body must not be
  // empty`).
  await form.getByLabel("Commit", { exact: true }).fill(commit);
  await form.getByLabel("Move to").selectOption("review");
  await expect(
    form.getByRole("button", { name: "Publish revision" }),
  ).toBeDisabled();

  await submitRevision(form, {
    source: session,
    commit,
    comment: "Ready for review",
    state: "review",
  });

  // The panel now answers the hand-off's one question: whose work, which
  // branch, which commit, and has anyone judged it.
  const current = handoffSection(panel);
  await expect(current.getByText(`session/${session}`)).toBeVisible({
    timeout: LIVE_TIMEOUT,
  });
  await expect(current.getByText(commit.slice(0, 7)).first()).toBeVisible();
  await expect(current.getByTitle(commit).first()).toBeVisible();
  await expect(current.getByText("Unreviewed").first()).toBeVisible();
  await expect(current.getByRole("link", { name: session.slice(0, 8) }).first()).toBeVisible();
  // The comment is written with the hand-off and shows on the thread.
  await expect(panel.getByText("Ready for review").first()).toBeVisible();

  // The record itself: a state change, a cleared lease and a fresh unreviewed
  // hand-off (`SPEC.md`, "Code hand-offs and review").
  const task = await getTask(fixture.client, fixture.project.id, 1);
  expect(task.state).toBe("review");
  expect(task.lease_holder_session_id).toBeNull();
  expect(task.handoff?.commit).toBe(commit);
  expect(task.handoff?.review_status).toBe("unreviewed");
  expect(task.handoff?.source_session_id).toBe(session);
  expect(task.handoff?.source_branch).toBe(`session/${session}`);

  // `ARCHITECTURE.md`, "Git model": the commit is retained under an immutable
  // ref of its own, so it survives whatever the branch does next.
  expect(gitRevParse(fixture.mirror, `refs/handoffs/${task.handoff?.id ?? ""}`)).toBe(
    commit,
  );
  // And the publication's own sync left the session ref at the same commit.
  expect(gitRevParse(fixture.mirror, `refs/sessions/${session}`)).toBe(commit);

  // The two 400s the form stands in front of, from the API itself.
  const noState = await fixture.client.send(
    "PUT",
    `/projects/${fixture.project.id}/tasks/1`,
    {
      handoff: {
        kind: "revision",
        source_session_id: session,
        commit,
        comment: "no state",
      },
    },
    { allow: [400] },
  );
  expect(noState.status).toBe(400);
  expect(noState.text).toContain("handoff requires a different target state");

  const noComment = await fixture.client.send(
    "PUT",
    `/projects/${fixture.project.id}/tasks/1`,
    {
      state: "merge",
      handoff: {
        kind: "revision",
        source_session_id: session,
        commit,
        comment: "   ",
      },
    },
    { allow: [400] },
  );
  expect(noComment.status).toBe(400);
  expect(noComment.text).toContain("comment body must not be empty");
});

test("a reviewer's session starts from the hand-off commit and is told about it", async ({
  context,
  request,
  browser,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const fixture = await stage(sessions, api, project);
  const session = await implementer(fixture);
  const commit = commitInSessionWorkClone(
    session,
    { "greeting.txt": "hello\n" },
    "feat: greeting",
  );
  const handoff = await publishRevision(
    fixture,
    session,
    commit,
    "Ready for review",
    "review",
  );

  const second = await reviewer(browser, request);
  const panel = await openTask(second.page, project);

  // `SPEC.md`, "Frontend", "Hand-off controls": the launch defaults to the
  // hand-off commit and says so, with nothing to disclose.
  let form = await openLaunchForm(panel);
  await expect(
    form.getByText(
      `Base: hand-off ${commit.slice(0, 7)} from session/${session} (unreviewed)`,
    ),
  ).toBeVisible();
  await expect(form.getByText(/Base overridden/)).toHaveCount(0);

  const reviewSession = await submitLaunch(second.page, form, sessions, second.client);

  // The session records the commit, not the branch (`SPEC.md`, "Sessions").
  const started = await second.client.get<Session>(`/sessions/${reviewSession}`);
  expect(started.base_ref).toBe(commit);
  expect(started.handoff_id).toBe(handoff.id);
  await expect(second.page.getByText(commit, { exact: true }).first()).toBeVisible({
    timeout: 60_000,
  });

  // The generated first message's second paragraph, which is how the agent
  // learns what it is looking at.
  const message = await reveal(
    second.page,
    transcript(second.page).getByText(
      new RegExp(`Current hand-off ${handoff.id}`),
    ),
  );
  const text = await message.innerText();
  expect(text).toContain(`on branch session/${session}`);
  expect(text).toContain(`at commit ${commit}`);
  expect(text).toContain("(review: unreviewed)");
  expect(text).toContain("Ready for review");

  // An explicit base overrides the hand-off, and the disclosure is the point:
  // starting somewhere else silently would hide the one thing a reviewer needs
  // to know. The first reviewer session is ended so the task is claimable
  // again.
  await endSession(second.client, reviewSession);
  await waitFor(
    async () => {
      const task = await getTask(fixture.client, fixture.project.id, 1);
      return task.lease_holder_session_id === null ? task : null;
    },
    {
      timeoutMs: 90_000,
      description: "the ended reviewer session to release the task",
    },
  );

  const again = await openTask(second.page, project);
  form = await openLaunchForm(again);
  await form.locator("summary").click();
  await form.getByLabel("Base ref").selectOption("main");
  await expect(
    form.getByText(
      "Base overridden: the session will not start from the hand-off commit and this grants no review approval",
    ),
  ).toBeVisible();

  const overridden = await submitLaunch(second.page, form, sessions, second.client);
  const third = await second.client.get<Session>(`/sessions/${overridden}`);
  expect(third.base_ref).toBe("main");
  // An explicit base is not a hand-off selection, so the session records none
  // (`SPEC.md`, "Sessions": "Otherwise `handoff_id` is null"). What it does
  // carry is the third paragraph, which says where the checkout really starts
  // and where the hand-off commit can still be fetched from.
  expect(third.handoff_id).toBeNull();
  const note = await reveal(
    second.page,
    transcript(second.page).getByText(/Your checkout starts from main/),
  );
  expect(await note.innerText()).toContain(`refs/handoffs/${handoff.id}`);
});

test("approving forwards the hand-off to merge and unlocks the task merge", async ({
  context,
  request,
  browser,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const fixture = await stage(sessions, api, project);
  const session = await implementer(fixture);
  const commit = commitInSessionWorkClone(
    session,
    { "greeting.txt": "hello\n" },
    "feat: greeting",
  );

  const second = await reviewer(browser, request);
  const panel = await openTask(second.page, project);
  await expect(handoffSection(panel).getByText("No code hand-off")).toBeVisible();

  // The board's live refresh reaches the open drawer: the hand-off U1 publishes
  // appears without a reload (ADR 0022; `SPEC.md`, "Board refresh ordering").
  const handoff = await publishRevision(
    fixture,
    session,
    commit,
    "Ready for review",
    "review",
  );
  await expect(
    handoffSection(panel).getByText(`session/${session}`),
  ).toBeVisible({ timeout: LIVE_TIMEOUT });
  // Nothing is approved yet, so the merge control is shut with its reason on it.
  await expect(mergeButton(panel)).toBeDisabled();

  const form = await openReviewForm(panel, "Approve");
  // The decision is about a commit, and the form says which one.
  await expect(
    form.getByText(`Approving commit ${commit.slice(0, 7)}`),
  ).toBeVisible();
  // An approval with an empty comment is refused by the form, as the API
  // requires one.
  await form.getByLabel("Move to").selectOption("merge");
  await expect(form.getByRole("button", { name: "Approve", exact: true })).toBeDisabled();

  await submitReview(form, "Approve", "merge", "LGTM");

  await expect(
    handoffSection(panel).getByText(`Approved · ${commit.slice(0, 7)}`).first(),
  ).toBeVisible({ timeout: LIVE_TIMEOUT });
  // The reviewer is named, by username (`GET /users/{id}`).
  await expect(
    handoffSection(panel).getByText(`@${second.user.username}`).first(),
  ).toBeVisible();
  await expect(mergeButton(panel)).toBeEnabled();

  const task = await getTask(fixture.client, fixture.project.id, 1);
  expect(task.state).toBe("merge");
  expect(task.handoff?.review_status).toBe("approved");
  expect(task.handoff?.reviewed_by_user_id).toBe(second.user.id);
  expect(task.handoff?.commit).toBe(commit);
  // Forwarding reuses the pinned commit and does not make a new revision of it.
  expect(task.handoff?.id).not.toBe(handoff.id);
  expect(task.handoffs).toHaveLength(2);

  // The board behind the drawer followed the move.
  await second.page.goto(`/projects/${fixture.project.id}?tab=board`);
  await expect(
    second.page.getByTestId("column-merge").getByTestId("task-card-1"),
  ).toBeVisible({ timeout: LIVE_TIMEOUT });
});

test("the task merge lands the pinned commit even after the branch advanced", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const fixture = await stage(sessions, api, project);
  const session = await implementer(fixture);
  const approved = commitInSessionWorkClone(
    session,
    { "greeting.txt": "hello\n" },
    "feat: greeting",
  );
  const handoff = await publishRevision(
    fixture,
    session,
    approved,
    "Ready for review",
    "review",
  );
  const forwarded = await forwardHandoff(
    fixture,
    fixture.client,
    handoff.id,
    "LGTM",
    "merge",
    "approved",
  );

  // The branch moves on after the approval, which is the normal case and
  // exactly what must not be merged (`SPEC.md`, "Code hand-offs and review").
  const later = commitInSessionWorkClone(
    session,
    { "greeting.txt": "hello again\n" },
    "more",
  );
  const synced = await syncSession(fixture.client, session);
  expect(synced.commit).toBe(later);

  const panel = await openTask(page, project);
  await mergeButton(panel).click();
  const form = panel.getByRole("form", { name: "Merge approved hand-off" });
  await expect(form).toBeVisible();
  await expect(
    form.getByText(
      `Merges commit ${approved.slice(0, 7)} exactly; later commits on session/${session} are not included`,
    ),
  ).toBeVisible();
  await expect(form.getByLabel("Target")).toHaveValue("main");

  await form.getByRole("button", { name: "Merge", exact: true }).click();
  await expect(
    form.getByText(/^Merged as [0-9a-f]{10} · Task state unchanged$/),
  ).toBeVisible({ timeout: 60_000 });

  // The pinned commit landed and the one after it did not.
  expect(gitIsAncestor(fixture.mirror, approved, "main")).toBe(true);
  expect(gitIsAncestor(fixture.mirror, later, "main")).toBe(false);
  expect(gitRevParse(fixture.mirror, `refs/handoffs/${forwarded.id}`)).toBe(
    approved,
  );

  // A merge is a git action, not a task move: the task is still in `merge`
  // until someone moves it, which the drawer can do.
  const afterMerge = await getTask(fixture.client, fixture.project.id, 1);
  expect(afterMerge.state).toBe("merge");

  await form.getByRole("button", { name: "Close" }).click();
  await panel.getByLabel("Move to").selectOption("done");
  await panel.getByRole("button", { name: "Move", exact: true }).click();
  await panel.getByRole("button", { name: "Move to done" }).click();
  await waitFor(
    async () => {
      const task = await getTask(fixture.client, fixture.project.id, 1);
      return task.state === "done" ? task : null;
    },
    { timeoutMs: LIVE_TIMEOUT, description: "the task to reach done" },
  );
});

test("the merge control is shut without an approval and a superseded review is refused", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const fixture = await stage(sessions, api, project);
  const session = await implementer(fixture);
  const first = commitInSessionWorkClone(
    session,
    { "greeting.txt": "hello\n" },
    "feat: greeting",
  );
  const original = await publishRevision(
    fixture,
    session,
    first,
    "Ready for review",
    "review",
  );

  const panel = await openTask(page, project);
  // `tasks/mergeRules.ts`: an unreviewed hand-off is not mergeable, and the
  // button carries the reason rather than sending a request that earns a 409.
  await expect(mergeButton(panel)).toBeDisabled();
  await expect(
    handoffSection(panel).getByTitle("Requires an approved current hand-off"),
  ).toBeVisible();

  // A second revision on the same task, published through the form.
  const second = commitInSessionWorkClone(
    session,
    { "greeting.txt": "hello there\n" },
    "fix: wording",
  );
  const form = await openRevisionForm(panel);
  await submitRevision(form, {
    source: session,
    commit: second,
    comment: "Second attempt",
    state: "ready",
  });

  await expect(
    handoffSection(panel).getByText(second.slice(0, 7)).first(),
  ).toBeVisible({ timeout: LIVE_TIMEOUT });
  await expect(handoffSection(panel).getByText("Unreviewed").first()).toBeVisible();
  // Both revisions are in the history, and only the newer one is current.
  await expect(
    handoffSection(panel).getByRole("listitem").filter({ hasText: "current" }),
  ).toHaveCount(1);
  await expect(handoffSection(panel).getByRole("listitem")).toHaveCount(2);
  await expect(mergeButton(panel)).toBeDisabled();

  // The superseded id is refused (`SPEC.md`: 409 `handoff_id is not the task's
  // current hand-off`).
  const refused = await fixture.client.send(
    "PUT",
    `/projects/${fixture.project.id}/tasks/1`,
    {
      state: "merge",
      handoff: {
        kind: "forward",
        handoff_id: original.id,
        comment: "stale approval",
        review: "approved",
      },
    },
    { allow: [409] },
  );
  expect(refused.status).toBe(409);
  expect(refused.text).toContain("handoff_id is not the task's current hand-off");

  const untouched = await getTask(fixture.client, fixture.project.id, 1);
  expect(untouched.state).toBe("ready");
  expect(untouched.handoff?.review_status).toBe("unreviewed");

  // The drawer's own approve forwards the current id and is accepted.
  const review = await openReviewForm(panel, "Approve");
  await submitReview(review, "Approve", "merge", "Looks right now");
  await expect(
    handoffSection(panel).getByText(`Approved · ${second.slice(0, 7)}`).first(),
  ).toBeVisible({ timeout: LIVE_TIMEOUT });
  await expect(mergeButton(panel)).toBeEnabled();
});

test("requesting changes sends the task back and a new revision resets the review", async ({
  page,
  context,
  request,
  browser,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const fixture = await stage(sessions, api, project);
  const session = await implementer(fixture);
  const commit = commitInSessionWorkClone(
    session,
    { "greeting.txt": "hello\n" },
    "feat: greeting",
  );
  await publishRevision(fixture, session, commit, "Ready for review", "review");

  const second = await reviewer(browser, request);
  const reviewPanel = await openTask(second.page, project);
  const form = await openReviewForm(reviewPanel, "Request changes");
  await expect(
    form.getByText(`Requesting changes on commit ${commit.slice(0, 7)}`),
  ).toBeVisible();
  await submitReview(form, "Request changes", "ready", "Please rename");

  await expect(
    handoffSection(reviewPanel)
      .getByText(`Changes requested · ${commit.slice(0, 7)}`)
      .first(),
  ).toBeVisible({ timeout: LIVE_TIMEOUT });

  const sent = await getTask(fixture.client, fixture.project.id, 1);
  expect(sent.state).toBe("ready");
  expect(sent.handoff?.review_status).toBe("changes_requested");
  expect(sent.handoff?.commit).toBe(commit);

  // A rejection still pins the commit the next implementer starts from.
  const launch = await openLaunchForm(reviewPanel);
  await expect(
    launch.getByText(
      `Base: hand-off ${commit.slice(0, 7)} from session/${session} (changes_requested)`,
    ),
  ).toBeVisible();
  await launch.getByRole("button", { name: "Cancel" }).click();

  // The fix, published as a new revision by the implementer, arrives unreviewed
  // however many decisions precede it.
  const fixed = commitInSessionWorkClone(
    session,
    { "greeting.txt": "hello, world\n" },
    "fix: rename",
  );
  const panel = await openTask(page, project);
  const revision = await openRevisionForm(panel);
  await submitRevision(revision, {
    source: session,
    commit: fixed,
    comment: "Renamed as asked",
    state: "review",
  });

  const after = await waitForTaskState(fixture, "review");
  await expect(
    handoffSection(panel).getByText(fixed.slice(0, 7)).first(),
  ).toBeVisible({ timeout: LIVE_TIMEOUT });
  await expect(handoffSection(panel).getByText("Unreviewed").first()).toBeVisible();
  expect(after.handoff?.commit).toBe(fixed);
  expect(after.handoff?.review_status).toBe("unreviewed");
  // The old decision is still in the history, on the commit it covered.
  expect(
    after.handoffs.filter((one) => one.review_status === "changes_requested"),
  ).toHaveLength(1);
});

test("the revision diff is read by hand-off id without syncing anything", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const fixture = await stage(sessions, api, project);
  const session = await implementer(fixture);
  const commit = commitInSessionWorkClone(
    session,
    { "greeting.txt": "hello\n" },
    "feat: greeting",
  );
  const handoff = await publishRevision(
    fixture,
    session,
    commit,
    "Ready for review",
    "review",
  );

  const panel = await openTask(page, project);
  await handoffSection(panel)
    .getByRole("button", { name: "View diff" })
    .first()
    .click();

  const diff = panel.getByRole("region", {
    name: `Diff of hand-off ${commit.slice(0, 7)}`,
  });
  await expect(diff).toBeVisible();
  await expect(
    diff.getByRole("button", { name: /^A greeting\.txt \+1 −0$/ }),
  ).toBeVisible({ timeout: 30_000 });
  await expect(diff.getByText("1 file")).toBeVisible();
  await expect(diff.getByText("hello", { exact: true }).first()).toBeVisible();

  // `SPEC.md`, "Git": the hand-off form of the diff selects the retained commit
  // without fetch-back, and the publication's own sync was silent — so the
  // implementer's transcript holds no git outcome at all.
  await page.goto(`/sessions/${session}`);
  await expect(transcript(page).getByText("Session started (stub)")).toBeVisible({
    timeout: 30_000,
  });
  await expect(transcript(page).getByText(/^Git .* succeeded$/)).toHaveCount(0);

  // The same answer over REST, which is what the panel asked for.
  const direct = await fixture.client.get<{ files: { path: string }[] }>(
    `/projects/${fixture.project.id}/git/diff?handoff_id=${handoff.id}`,
  );
  expect(direct.files.map((file) => file.path)).toEqual(["greeting.txt"]);
});

test("a commit that is not the source session's tip is refused", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const fixture = await stage(sessions, api, project);
  const session = await implementer(fixture);
  const tip = commitInSessionWorkClone(
    session,
    { "greeting.txt": "hello\n" },
    "feat: greeting",
  );

  const panel = await openTask(page, project);
  const form = await openRevisionForm(panel);
  // A well-formed object id that is nothing: the form lets it through, because
  // the tip is the server's fact to check (`SPEC.md`: 409 `session branch tip
  // <tip> does not match commit <commit>`).
  const invented = "0123456789abcdef0123456789abcdef01234567";
  await submitRevision(form, {
    source: session,
    commit: invented,
    comment: "Ready for review",
    state: "review",
  });

  const refusal = form
    .getByRole("alert")
    .filter({ hasText: "does not match commit" });
  await expect(refusal).toBeVisible({ timeout: 30_000 });
  await expect(refusal).toContainText(invented);

  // The refusal is a stop: the task keeps its state and its lease.
  const task = await getTask(fixture.client, fixture.project.id, 1);
  expect(task.state).toBe("ready");
  expect(task.lease_holder_session_id).toBe(session);
  expect(task.handoff).toBeNull();

  // An abbreviation never reaches the server at all: the form answers it in the
  // API's own words (`tasks/handoffRules.ts`).
  await form.getByLabel("Commit", { exact: true }).fill(tip.slice(0, 10));
  await form.getByRole("button", { name: "Publish revision" }).click();
  await expect(
    form.getByText("commit must be a full lowercase hexadecimal git object id"),
  ).toBeVisible();
});

test("a review of a superseded revision says the hand-off changed", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const fixture = await stage(sessions, api, project);
  const session = await implementer(fixture);
  const first = commitInSessionWorkClone(
    session,
    { "greeting.txt": "hello\n" },
    "feat: greeting",
  );
  const original = await publishRevision(
    fixture,
    session,
    first,
    "Ready for review",
    "review",
  );

  // The drawer refetches the task on every task event, so a second revision
  // published while the review form is open would normally reach that form
  // first and the submit would forward the *new* id. The task read is frozen
  // on its first answer instead: the page holds exactly what a reviewer whose
  // decision was already in flight holds, and the submit really does carry a
  // superseded `handoff_id` (`SPEC.md`, "Code hand-offs and review": a forward
  // must name the current one).
  const detail = `**/api/projects/${project.id}/tasks/1`;
  let frozen: string | null = null;
  await page.route(detail, async (route) => {
    if (route.request().method() !== "GET") {
      await route.fallback();
      return;
    }
    frozen ??= await (await route.fetch()).text();
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: frozen,
    });
  });

  const panel = await openTask(page, project);
  const form = await openReviewForm(panel, "Approve");
  await expect(
    form.getByText(`Approving commit ${first.slice(0, 7)}`),
  ).toBeVisible();

  const second = commitInSessionWorkClone(
    session,
    { "greeting.txt": "hello there\n" },
    "fix: wording",
  );
  // Back to `ready`: a hand-off has to move the task somewhere it is not
  // (`SPEC.md`, "Code hand-offs and review": 400 `handoff requires a different
  // target state`).
  const superseding = await publishRevision(
    fixture,
    session,
    second,
    "Second attempt",
    "ready",
  );
  expect(superseding.id).not.toBe(original.id);

  await submitReview(form, "Approve", "merge", "Looks right");

  // The one refusal the server's own words would not help with: retrying
  // cannot succeed, and the answer is to read the revision that arrived
  // (`tasks/handoffRules.ts`, `reviewErrorMessage`).
  await expect(
    form.getByText("The hand-off changed; review the new revision"),
  ).toBeVisible({ timeout: 30_000 });

  // Nothing was decided: the current hand-off is still the newer one, and it
  // is still unreviewed.
  await page.unroute(detail);
  const task = await getTask(fixture.client, fixture.project.id, 1);
  expect(task.handoff?.id).toBe(superseding.id);
  expect(task.handoff?.review_status).toBe("unreviewed");
  expect(task.state).toBe("ready");
});
