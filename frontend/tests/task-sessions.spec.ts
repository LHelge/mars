// Where the tracker and the sessions meet (`SPEC.md`, "User-facing features",
// "Sessions" and "Task board"; "Sessions" (`POST /projects/{pid}/sessions`
// with `task_id`); "Tasks" (release); "Frontend", "Task board" and
// "Dashboard"; `ARCHITECTURE.md`, "Task tracker" → "Launching a session for a
// task", "Liveness comes from the session, not from tool calls" and "Attempts
// and escalation").
//
// One sentence per scenario: opening a task in a session claims it and the
// card says so, a held task cannot be opened again, releasing from the drawer
// gives it back without escalating, ending the session gives it back with a
// system comment, "run once" runs an ephemeral profile on it, the launch form
// discloses the base it starts from, a task waiting for a person shows on the
// dashboard, an escalation at the attempt limit emails the assignee, and the
// dashboard lists sessions across projects.
//
// **Two places where the contract is not what the task's acceptance criteria
// assumed**, both resolved in favour of the documents and the landed code:
//
//   - A *user* moving a task into the human state sends no email
//     (`ARCHITECTURE.md`, "Task tracker" → "Notification": "a user moving a
//     task into the human state by hand sends none"). The escalation email is
//     therefore asserted on the path that owes one and is reachable without an
//     agent: a release at the project's `max_attempts`, which the orchestrator
//     performs itself when the holding session ends.
//   - An ephemeral session's generated task message is the head of its `-p`
//     prompt and is *not* recorded as a `user_message` (`SPEC.md`,
//     "Sessions"), so only a conversational session's transcript can show it.
//
// The stub image calls no MCP tool at all (`images/stub/claude`), so nothing
// an agent does to a task — claim, hand-off, `needs_human` — is reachable from
// here; every task state in this file is produced by the orchestrator itself
// or by the user, as a user would.

import type { Locator, Page } from "@playwright/test";

import type { Profile, Project, Session } from "../src/types";
import { apiClient, expect, test } from "./utils/fixtures";
import type { SessionTracker } from "./utils/fixtures";
import {
  createBareRepo,
  createProject,
  createTask,
  createTestUser,
  defaultProfile,
  endSession,
  getTask,
  loggedEmail,
  loginViaToken,
  logOffset,
  reveal,
  setProfileSecrets,
  setProjectSecret,
  sleep,
  taskCardTestId,
  transcript,
  waitFor,
  waitForSessionState,
  type Api,
} from "./utils/test-helpers";

// The upstream every scenario here clones: the file the fixture's `Edit` tool
// rewrites.
test.use({ repoFiles: { "src/app.py": 'def main():\n    print("hello")\n' } });

// A container start, a git clone and a replayed turn or two, as in
// `sessions.spec.ts`.
test.setTimeout(180_000);

/** A live board refresh is coalesced, so nothing here asserts immediacy. */
const LIVE_TIMEOUT = 10_000;

// --- the board and its drawer -----------------------------------------------

function boardPath(project: Project): string {
  return `/projects/${project.id}?tab=board`;
}

function card(page: Page, number: number): Locator {
  return page.getByTestId(taskCardTestId(number));
}

/** The task drawer of `/projects/:id/tasks/:number`. */
function drawer(page: Page): Locator {
  return page.getByRole("dialog");
}

/** Opens the drawer by its own route, which is the board with it open. */
async function openTask(
  page: Page,
  project: Project,
  number: number,
): Promise<Locator> {
  await page.goto(`/projects/${project.id}/tasks/${String(number)}`);
  const panel = drawer(page);
  await expect(panel).toHaveAttribute("aria-label", `Task #${String(number)}`);
  return panel;
}

/** The card's own holder link, the `held` chip of `TaskCard`. */
function holderLink(page: Page, number: number): Locator {
  // `exact`, because the card itself is a link whose accessible name contains
  // the task's title and would otherwise match a substring search.
  return card(page, number).getByRole("link", { name: "held", exact: true });
}

/** Opens one of the drawer's two launch forms and returns it. */
async function openLaunchForm(
  panel: Locator,
  action: "Open in session" | "Run once",
): Promise<Locator> {
  await panel.getByRole("button", { name: action, exact: true }).click();
  const form = panel.getByRole("form", { name: action });
  await expect(form).toBeVisible();
  return form;
}

/**
 * Submits an open launch form and returns the session the app navigated to,
 * registered for the `afterEach` clean-up.
 */
async function submitLaunch(
  page: Page,
  form: Locator,
  tracker: SessionTracker,
  client: Api,
  action: "Open in session" | "Run once",
): Promise<string> {
  await form.getByRole("button", { name: action, exact: true }).click();
  await page.waitForURL(/\/sessions\/[0-9a-f-]{8}-/);
  const id = page.url().slice(page.url().lastIndexOf("/") + 1);
  return tracker.track(client, id);
}

/** Launches a session for a task over REST, without driving the drawer. */
function claimWithSession(
  tracker: SessionTracker,
  client: Api,
  project: Project,
  taskNumber: number,
): Promise<Session> {
  return tracker.launch(client, project.id, { task_id: taskNumber });
}

// --- the session view -------------------------------------------------------

/**
 * The side panel's `Tasks` tab, opened and returned as its `Launched for`
 * section — the task the session was launched holding, which the panel takes
 * out of the touched list and so lists exactly once (`SPEC.md`, "Frontend",
 * "Tasks panel"; `session/TasksPanel.tsx`).
 */
async function openLaunchedForPanel(page: Page): Promise<Locator> {
  await page.getByRole("tab", { name: "Tasks" }).click();
  const launchedFor = page
    .getByRole("tabpanel")
    .locator("section")
    .filter({ has: page.getByText("Launched for") });
  await expect(launchedFor).toBeVisible();
  return launchedFor;
}

// --- the dashboard ----------------------------------------------------------

/** One of the dashboard's three lists, by its heading. */
function dashboardSection(page: Page, title: string): Locator {
  return page
    .locator("section")
    .filter({ has: page.getByRole("heading", { name: title, exact: true }) });
}

// --- scenarios --------------------------------------------------------------

test("open in session claims the task and the card shows its session", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  await createTask(api, project.id, {
    title: "Implement greeting",
    state: "ready",
  });
  // The form defaults to the first conversational profile that serves the
  // task's state (`SPEC.md`, "Frontend", "Task board";
  // `tasks/launchRules.ts`). None of the seeded ones does — `claude` serves
  // nothing, the planner `backlog`, and the implementer over `ready` is
  // ephemeral (ADR 0051) — so the scenario brings its own, and the rule is
  // what picks it over `claude`, which comes first in the list.
  const profile = await api.post<Profile>(`/projects/${project.id}/profiles`, {
    name: "pair",
    kind: "conversational",
    serves_states: ["ready"],
  });

  const panel = await openTask(page, project, 1);
  const form = await openLaunchForm(panel, "Open in session");

  await expect(form.getByLabel("Agent profile")).toHaveValue(profile.id);
  await expect(
    form.getByLabel("Agent profile").locator(`option[value="${profile.id}"]`),
  ).toHaveText(`${profile.name} — serves ready`);

  const sessionId = await submitLaunch(
    page,
    form,
    sessions,
    api,
    "Open in session",
  );

  // The title defaults to the task's own (`SPEC.md`, "Sessions").
  await expect(
    page.locator("main header").getByRole("button", { name: "Edit title" }),
  ).toHaveText("Implement greeting");

  // The generated first message, delivered ahead of anything a user typed and
  // recorded as a `user_message` with no author.
  await reveal(
    page,
    transcript(page).getByText(/^You hold task #1: Implement greeting\./),
  );

  const tasks = await openLaunchedForPanel(page);
  await expect(
    tasks.getByRole("link", { name: /Implement greeting/ }),
  ).toBeVisible();
  await expect(tasks.getByText("held by this session")).toBeVisible();
  // Once, not twice: a claim writes the session link, so the launched task is
  // in the touched list the panel reads, and it is shown from there.
  await expect(
    page
      .getByRole("tabpanel")
      .getByRole("link", { name: /Implement greeting/ }),
  ).toHaveCount(1);

  // The claim itself: the lease, the count and the card's link back.
  const claimed = await getTask(api, project.id, 1);
  expect(claimed.lease_holder_session_id).toBe(sessionId);
  expect(claimed.attempts).toBe(1);
  expect(claimed.state).toBe("ready");

  await page.goto(boardPath(project));
  await expect(holderLink(page, 1)).toHaveAttribute(
    "href",
    `/sessions/${sessionId}`,
  );

  const reopened = await openTask(page, project, 1);
  await expect(reopened.getByRole("button", { name: "Release" })).toBeEnabled();
  // The drawer's sessions list marks the one holding it now.
  await expect(reopened.getByText("holding", { exact: true })).toBeVisible();
});

test("a held task cannot be opened in a second session", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  await createTask(api, project.id, { title: "Only once", state: "ready" });
  const session = await claimWithSession(sessions, api, project, 1);

  const panel = await openTask(page, project, 1);

  // The drawer mirrors the API's refusal before the request rather than after
  // it: both launch buttons carry the reason and neither can be pressed
  // (`tasks/launchRules.ts`).
  for (const action of ["Open in session", "Run once"]) {
    const button = panel.getByRole("button", { name: action, exact: true });
    await expect(button).toBeDisabled();
    await expect(button).toHaveAttribute(
      "title",
      "Held by a session; release it first",
    );
  }

  // The server stays the authority, and says the same thing (`SPEC.md`,
  // "Sessions": 409 if `task_id` names a task that is held).
  const profile = await defaultProfile(api, project.id);
  const refused = await api.send(
    "POST",
    `/projects/${project.id}/sessions`,
    { profile_id: profile.id, task_id: 1 },
    { allow: [409] },
  );
  expect(refused.status).toBe(409);
  expect(refused.text).toContain("task is not claimable");

  const held = await api.get<Session[]>(`/projects/${project.id}/sessions`);
  expect(held.map((row) => row.id)).toEqual([session.id]);
});

test("release from the drawer clears the claim without escalating", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  await createTask(api, project.id, {
    title: "Give it back",
    state: "ready",
  });
  const session = await claimWithSession(sessions, api, project, 1);

  const panel = await openTask(page, project, 1);
  await expect(holderLink(page, 1)).toBeVisible();

  await panel.getByRole("button", { name: "Release" }).click();

  await expect(holderLink(page, 1)).toHaveCount(0, { timeout: LIVE_TIMEOUT });
  await expect(panel.getByRole("button", { name: "Release" })).toBeDisabled();

  // A user release keeps the state and never escalates, and `attempts` stays
  // where the claim put it (`SPEC.md`, "Tasks").
  const released = await getTask(api, project.id, 1);
  expect(released.lease_holder_session_id).toBeNull();
  expect(released.state).toBe("ready");
  expect(released.attempts).toBe(1);

  // The session is untouched by its task being taken away.
  const still = await api.get<Session>(`/sessions/${session.id}`);
  expect(still.ended_at).toBeNull();
  expect(["creating", "running"]).toContain(still.state);
});

test("ending the session releases its task and says so on the thread", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  await createTask(api, project.id, {
    title: "Held to the end",
    state: "ready",
  });
  const session = await claimWithSession(sessions, api, project, 1);
  await waitForSessionState(api, session.id, "running", 90_000);

  await openTask(page, project, 1);
  await expect(holderLink(page, 1)).toBeVisible();

  await endSession(api, session.id);
  await waitForSessionState(api, session.id, ["done", "failed"], 90_000);

  // The session hooks release on the spot; the stuck-task reaper is only the
  // backstop (`ARCHITECTURE.md`, "Session lifecycle"; "Task tracker" →
  // "Liveness comes from the session, not from tool calls").
  const released = await waitFor(
    async () => {
      const task = await getTask(api, project.id, 1);
      return task.lease_holder_session_id === null ? task : null;
    },
    {
      timeoutMs: LIVE_TIMEOUT,
      description: "the ended session's lease to be released",
    },
  );
  expect(released.state).toBe("ready");

  await expect(holderLink(page, 1)).toHaveCount(0, { timeout: LIVE_TIMEOUT });

  const panel = await openTask(page, project, 1);
  // The system comment the orchestrator writes, word for word.
  await expect(
    panel.getByText(
      `Lease released by the orchestrator: holder session ${session.id} ended.`,
    ),
  ).toBeVisible();
  // And the drawer's sessions list still links into the transcript of the
  // session that held it.
  await expect(
    panel.getByRole("link", { name: session.id.slice(0, 8) }),
  ).toHaveAttribute("href", `/sessions/${session.id}`);
});

test("run once runs an ephemeral profile on the task and gives it back", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  await createTask(api, project.id, {
    title: "Run me once",
    state: "ready",
  });
  // The scenario's own ephemeral profile, rather than the seeded `implementer`
  // that also serves `ready` (ADR 0051): what runs here is arranged here.
  const oneshot = await api.post<Profile>(`/projects/${project.id}/profiles`, {
    name: "oneshot",
    kind: "ephemeral",
    serves_states: ["ready"],
  });

  const panel = await openTask(page, project, 1);
  const form = await openLaunchForm(panel, "Run once");
  // The form offers only ephemeral profiles and starts on the first that
  // serves the task's state, which is the older seeded implementer; choosing
  // is the user's.
  await form.getByLabel("Agent profile").selectOption(oneshot.id);
  await expect(form.getByLabel("Agent profile")).toHaveValue(oneshot.id);

  const sessionId = await submitLaunch(page, form, sessions, api, "Run once");

  // An ephemeral session runs one prompt and takes no further input, so there
  // is no composer at all (`SPEC.md`, "Frontend", "Composer").
  await expect(page.getByLabel("Message", { exact: true })).toHaveCount(0);

  // Its generated task message is the head of the `-p` prompt and is not
  // recorded as a `user_message` (`SPEC.md`, "Sessions"), so the transcript
  // starts with the run itself; that the session was launched holding the task
  // is asserted through the side panel and the API instead.
  const tasks = await openLaunchedForPanel(page);
  await expect(tasks.getByRole("link", { name: /Run me once/ })).toBeVisible();
  // Not "held by this session": a one-shot over the stub's fixture is often
  // finished — and its lease therefore given back — before the panel is even
  // opened. That the session held the task is the release assertion below.
  await reveal(page, transcript(page).getByText("Session started (stub)"));

  // The owner marks an ephemeral session `done` when the first `result`
  // arrives (`ARCHITECTURE.md`, "Claude Code invocation"), and ending releases
  // the task.
  const done = await waitForSessionState(api, sessionId, "done", 120_000);
  expect(done.task_id).not.toBeNull();

  const released = await waitFor(
    async () => {
      const task = await getTask(api, project.id, 1);
      return task.lease_holder_session_id === null ? task : null;
    },
    {
      timeoutMs: LIVE_TIMEOUT,
      description: "the finished one-shot to release its task",
    },
  );
  expect(released.state).toBe("ready");
  // The claim it made on the way is still on the record: one attempt, and the
  // session linked to the task (`docs/data-model.md`, `task_sessions`).
  expect(released.attempts).toBe(1);
  expect(released.sessions.map((touch) => touch.session_id)).toContain(
    sessionId,
  );

  await expect(page.getByText("done", { exact: true }).first()).toBeVisible();
});

test("the launch form discloses the base the session will start from", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await loginViaToken(context, user);
  await createTask(api, project.id, {
    title: "No hand-off",
    state: "ready",
  });

  const panel = await openTask(page, project, 1);
  const form = await openLaunchForm(panel, "Open in session");

  // Without a hand-off the base is the project's default branch and there is
  // nothing to disclose (`SPEC.md`, "Frontend", "Hand-off controls"; the
  // hand-off variant of this sentence lives in the hand-off spec).
  await expect(
    form.getByText(`Base: ${project.default_branch ?? "main"}`),
  ).toBeVisible();
  await expect(form.getByText(/Base overridden/)).toHaveCount(0);

  // Choosing one is what discloses it, and the picker is deliberately behind
  // a disclosure the user has to open first (`tasks/LaunchForTask.tsx`). It is
  // the project page's control: the mirror's refs, plus a custom entry for a
  // tag or a commit id.
  await form.locator("summary").click();
  await form.getByLabel("Base ref").selectOption({ label: "Custom ref\u2026" });
  await form.getByLabel("Custom base ref").fill("origin/main");
  await expect(form.getByText(/Base overridden/)).toBeVisible();
});

test("a task moved into needs_human shows on the dashboard", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await loginViaToken(context, user);
  const title = `Decide the schema ${String(Date.now())}`;
  await createTask(api, project.id, { title, state: "ready" });

  const panel = await openTask(page, project, 1);
  await panel.getByLabel("Move to").selectOption("needs_human");
  await panel.getByRole("button", { name: "Move", exact: true }).click();
  await panel.getByRole("button", { name: "Move to needs_human" }).click();

  await expect(
    page.getByTestId("column-needs_human").getByTestId("task-card-1"),
  ).toBeVisible({
    timeout: LIVE_TIMEOUT,
  });

  // `GET /tasks?state_kind=human` is across projects, so the row is found by
  // this scenario's own task, never by a count.
  await page.goto("/");
  const human = dashboardSection(page, "Needs a human");
  const row = human.getByRole("link", { name: title });
  await expect(row).toBeVisible();
  await expect(row).toHaveAttribute("href", `/projects/${project.id}/tasks/1`);

  // A user's own move is not an escalation: it emits `state_changed`, not
  // `escalated`, and owes no email (`ARCHITECTURE.md`, "Task tracker" →
  // "Notification"). The escalation that does owe one is the next scenario.
  const moved = await getTask(api, project.id, 1);
  expect(moved.state).toBe("needs_human");
  expect(moved.needs_human_reason).toBeNull();
});

test("an escalation at the attempt limit emails the assignee, and not one who opted out", async ({
  request,
  user,
  api,
  project,
  sessions,
}) => {
  // One attempt, so the first release the orchestrator makes is the one that
  // escalates (`SPEC.md`, "Tasks"; `ARCHITECTURE.md`, "Attempts and
  // escalation").
  await api.put(`/projects/${project.id}`, { max_attempts: 1 });

  const quiet = await createTestUser(request, { prefix: "quiet" });
  const quietClient = apiClient(request, quiet.access_token);
  await quietClient.patch("/users/me", { notify_email: false });

  const mine = await createTask(api, project.id, {
    title: "Escalate to me",
    state: "ready",
  });
  const theirs = await createTask(api, project.id, {
    title: "Escalate to nobody",
    state: "ready",
  });
  await api.put(`/projects/${project.id}/tasks/${mine.number}`, {
    assignee_user_id: user.id,
  });
  await api.put(`/projects/${project.id}/tasks/${theirs.number}`, {
    assignee_user_id: quiet.id,
  });

  const off = logOffset();

  for (const task of [mine, theirs]) {
    const session = await claimWithSession(sessions, api, project, task.number);
    await waitForSessionState(api, session.id, "running", 90_000);
    await endSession(api, session.id);
    await waitForSessionState(api, session.id, ["done", "failed"], 90_000);
  }

  // Both tasks are escalated by the release, whoever is told about it.
  for (const task of [mine, theirs]) {
    const escalated = await waitFor(
      async () => {
        const row = await getTask(api, project.id, task.number);
        return row.state === "needs_human" ? row : null;
      },
      {
        timeoutMs: LIVE_TIMEOUT,
        description: `task #${String(task.number)} to be escalated`,
      },
    );
    expect(escalated.needs_human_reason).toContain(
      "attempt limit reached (1/1)",
    );
  }

  // The assignee is the sole recipient and is told; the assignee who turned
  // `notify_email` off is told nothing, and the administrators are not fallen
  // back to (`ARCHITECTURE.md`, "Task tracker" → "Notification").
  await waitFor(
    () => loggedEmail(user.email, `Task #1 needs a human: ${mine.title}`, off),
    {
      timeoutMs: 15_000,
      description: `the escalation email for ${user.email}`,
    },
  );
  // Five seconds after its own escalation landed, so the absence is an
  // absence and not a race.
  await sleep(5_000);
  expect(
    loggedEmail(quiet.email, `Task #2 needs a human: ${theirs.title}`, off),
  ).toBe(false);
});

test("the dashboard lists running and parked sessions across projects", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const secondRepo = createBareRepo("dash-b");
  const second = await createProject(api, {
    remote_url: secondRepo.url,
  });

  const runningTitle = `Still running ${String(Date.now())}`;
  const parkedTitle = `Parked here ${String(Date.now())}`;

  const running = await sessions.launch(api, project.id, {
    title: runningTitle,
    message: "hello stub",
  });
  const parked = await sessions.launch(api, second.id, {
    title: parkedTitle,
    message: "hello stub",
  });

  await waitForSessionState(api, running.id, "running", 90_000);
  await waitForSessionState(api, parked.id, "running", 90_000);
  await api.send("POST", `/sessions/${parked.id}/stop`);
  await waitForSessionState(api, parked.id, "parked", 90_000);

  await page.goto("/");

  const runningRow = dashboardSection(page, "Running sessions").getByRole(
    "link",
    { name: runningTitle },
  );
  await expect(runningRow).toHaveAttribute("href", `/sessions/${running.id}`);
  await expect(
    dashboardSection(page, "Running sessions")
      .getByRole("row")
      .filter({ hasText: runningTitle }),
  ).toContainText(project.name);

  const parkedRow = dashboardSection(page, "Parked sessions").getByRole(
    "link",
    {
      name: parkedTitle,
    },
  );
  await expect(parkedRow).toHaveAttribute("href", `/sessions/${parked.id}`);
  await expect(
    dashboardSection(page, "Parked sessions")
      .getByRole("row")
      .filter({ hasText: parkedTitle }),
  ).toContainText(second.name);

  // The list is polled every 30 seconds; navigating away and back is the
  // cheaper way to see the next answer (`SPEC.md`, "Frontend", Dashboard).
  await endSession(api, running.id);
  await waitForSessionState(api, running.id, ["done", "failed"], 90_000);
  await page.goto("/projects");
  await page.goto("/");
  await expect(
    dashboardSection(page, "Running sessions").getByRole("link", {
      name: runningTitle,
    }),
  ).toHaveCount(0, { timeout: LIVE_TIMEOUT });
});

// --- no fact only in a tooltip ----------------------------------------------

test("a failed session's reason is visible on a phone without a tooltip @mobile", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  // The stub's failing run: one turn, then exit 1, which the orchestrator
  // records as `failed` with the reason in `error` (`ARCHITECTURE.md`,
  // "Session lifecycle").
  await setProjectSecret(api, project.id, "MARS_STUB_EXIT_AFTER_TURNS", "1");
  await setProjectSecret(api, project.id, "MARS_STUB_EXIT_CODE", "1");
  await setProfileSecrets(api, project.id, [
    "MARS_STUB_EXIT_AFTER_TURNS",
    "MARS_STUB_EXIT_CODE",
  ]);
  const session = await sessions.launch(api, project.id, {
    message: "fail please",
  });
  const failed = await waitForSessionState(api, session.id, "failed", 90_000);
  const reason = String(failed.error);
  expect(failed.error).not.toBeNull();

  await page.goto(`/projects/${project.id}?tab=sessions`);
  const row = page.getByRole("row").filter({
    has: page.getByRole("link", { name: String(failed.title) }),
  });
  // Text on the row, on a phone, with nothing to hover (`SPEC.md`,
  // "Frontend", "Mobile layout").
  const text = row.getByText(reason);
  await expect(text).toBeVisible();
  await expect(text).not.toHaveAttribute("title");
});
