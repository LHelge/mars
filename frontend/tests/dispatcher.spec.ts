// Automatic dispatch, end to end in a browser (`SPEC.md`, "User-facing
// features" → "Automatic dispatch"; "Frontend" → "Unattended launches" and
// "Launch source"; `ARCHITECTURE.md`, "Task tracker" → "Unattended launches"
// and "Dispatcher"; ADR 0042).
//
// The one scenario here is the loop closing with nobody in it: a user ticks
// `auto_launch` on an ephemeral profile, drops a task into a state that
// profile serves, and a session appears — on the task, in the project's
// session list and in its own header — that no launch form ever submitted.
// Then the user pauses automation and drops the second task in, and nothing
// starts until the pause is lifted.
//
// **What the scenario waits on.** Not the sweep. The dispatcher subscribes to
// the task-event fan-out and runs 250 ms after a notice, so the move made in
// the drawer is what starts the session, normally inside a second
// (`orchestrator/src/cron/dispatcher.rs`). `DISPATCHER_INTERVAL_SECS=10` in
// `tests/e2e-stack.sh` is the fallback under that, and it is what the *resume*
// below leans on: lifting the pause writes no task event, so the next timer
// tick is what notices.
//
// **What the pause assertion rests on.** A job not running is not something an
// API can be asked about, so the claim is made out of three facts together:
// the move into the served state is a notice the dispatcher provably receives
// and acts on — it did so for the first task in this very scenario — the
// project still has exactly one session and the second task is unclaimed after
// the first session has run a container to completion, and the launch happens
// within a tick of the resume. Nothing sleeps for a fixed period at any point.
//
// **What the stub cannot do, and what the scenario does about it.** The stub
// image replays a fixture and calls no MCP tool at all
// (`images/stub/claude`), so a dispatched agent never hands its task on: when
// the session ends, the task comes straight back to the state it was claimed
// from — which an `auto_launch` profile serves, so it would be dispatched
// again, and again, until the project's `max_attempts` escalated it. The
// project here therefore runs with `max_attempts: 1`, so the first release is
// the escalation: one run per task, `needs_human` afterwards, and a board that
// stands still to be asserted on. That is v1 behaviour and not an arrangement
// around the dispatcher — `task-sessions.spec.ts` › `an escalation at the
// attempt limit …` is the same path with a person's session.
//
// **Clean-up.** This is the one spec whose sessions nobody here launched, so it
// registers the project with `sessions.sweep`: at teardown the project's
// automation is paused and every session it has is ended, whether or not this
// file ever learnt its id (`tests/utils/fixtures.ts`).

import type { Locator, Page } from "@playwright/test";

import type { Profile, Project, Session, TaskDetail } from "../src/types";
import { expect, test } from "./utils/fixtures";
import {
  FAKE_AGENT_CREDENTIAL,
  LAUNCH_SOURCE,
  createTask,
  getTask,
  listProjectSessions,
  loginViaToken,
  setProjectSecret,
  turnOffAutoLaunch,
  waitFor,
  waitForSessionState,
  type Api,
} from "./utils/test-helpers";

// A container start, a git clone and a replayed turn, as in `sessions.spec.ts`
// — twice over, since the resume launches a second session.
test.setTimeout(240_000);

/**
 * The project-scope agent credential the scenario stores, deleted after it.
 *
 * Unlike the user-scope one every spec gets from the `api` fixture, a
 * project-scope credential is on `/secrets` for *every* user — the page lists
 * every project's (`SPEC.md`, "Frontend", Agent credentials) — so one left
 * behind puts a second `Claude subscription token` row in front of
 * `secrets.spec.ts`, which addresses that row by its label.
 */
const credentials: { client: Api; id: string }[] = [];

test.afterEach(async () => {
  for (const entry of credentials.splice(0, credentials.length)) {
    await entry.client.delete(`/secrets/${entry.id}`, undefined, {
      allow: [403, 404],
    });
  }
});

/** The state the profile below serves, and the one both tasks are moved into. */
const SERVED = "ready";

/**
 * How long a dispatched launch is given.
 *
 * A wake-up run starts within a second of the move; the resume waits out a
 * timer tick first. Either way the budget is mostly the launch itself — the
 * mirror clone and the container start — which is what the session helpers
 * allow 60 s for.
 */
const DISPATCH_TIMEOUT = 90_000;

/** The project page's task drawer, at `/projects/:id/tasks/:number`. */
async function openTask(
  page: Page,
  project: Project,
  number: number,
): Promise<Locator> {
  await page.goto(`/projects/${project.id}/tasks/${String(number)}`);
  const panel = page.getByRole("dialog");
  await expect(panel).toHaveAttribute("aria-label", `Task #${String(number)}`);
  return panel;
}

/** Moves the open task through the drawer's `Move to` control. */
async function moveFromDrawer(panel: Locator, to: string): Promise<void> {
  await panel.getByLabel("Move to").selectOption(to);
  await panel.getByRole("button", { name: "Move", exact: true }).click();
  await panel.getByRole("button", { name: `Move to ${to}` }).click();
}

/** The project settings form, reached from the project page's header. */
async function openSettings(page: Page, project: Project): Promise<Locator> {
  await page.goto(`/projects/${project.id}`);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const form = page.getByRole("form", { name: "Project settings" });
  await expect(form).toBeVisible();
  return form;
}

/** Sets the `Pause automation` toggle and saves. */
async function setPaused(
  page: Page,
  project: Project,
  paused: boolean,
): Promise<void> {
  const form = await openSettings(page, project);
  const toggle = form.getByRole("checkbox", { name: /Pause automation/ });
  if (paused) {
    await toggle.check();
  } else {
    await toggle.uncheck();
  }
  await form.getByRole("button", { name: "Save settings" }).click();
  await expect(form.getByText("Settings saved.")).toBeVisible();
}

/** The session the dispatcher launched for `taskId`, once there is one. */
function waitForDispatched(
  client: Api,
  project: Project,
  taskId: string,
): Promise<Session> {
  return waitFor(
    async () => {
      const sessions = await listProjectSessions(client, project.id);
      return (
        sessions.find(
          (session) =>
            session.launch_source === "dispatcher" &&
            session.task_id === taskId,
        ) ?? null
      );
    },
    {
      timeoutMs: DISPATCH_TIMEOUT,
      intervalMs: 250,
      description: `the dispatcher to launch a session for task ${taskId}`,
    },
  );
}

/**
 * The task once the finished session's release has escalated it.
 *
 * The lease goes back through `AppState::session_ended` and the tracker hook
 * on it, and at `max_attempts` that release hands the task to a person
 * (`ARCHITECTURE.md`, "Task tracker" → "Attempts and escalation"). The move
 * into the human state resets `attempts`, so what a claim left behind is read
 * off `needs_human_reason` and the session list rather than off the counter
 * (`SPEC.md`, "Tasks").
 */
function escalatedTask(
  client: Api,
  project: Project,
  number: number,
): Promise<TaskDetail> {
  return waitFor(
    async () => {
      const task = await getTask(client, project.id, number);
      return task.state === "needs_human" ? task : null;
    },
    {
      description: `task #${String(number)} to be escalated by its dispatched session`,
    },
  );
}

test("the dispatcher picks up a task moved into a served state, and a pause stops the next one", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  // Everything this scenario leaves behind belongs to the project, including
  // the sessions it never names.
  sessions.sweep(api, project.id);

  // The seeded `implementer` over `ready` and `reviewer` over `review` carry
  // `auto_launch` already (ADR 0051), and are older than the profile below, so
  // with the credential stored the dispatcher would run the implementer on the
  // first task instead. Off before the credential exists, so no run of the job
  // ever sees them live (`tests/README.md`, "Automation and the seeded
  // roles"); the toggle this scenario is about is then the only one on.
  const seeded = await turnOffAutoLaunch(api, project.id);
  expect(seeded.map((one) => one.name).sort()).toEqual([
    "implementer",
    "reviewer",
  ]);

  // An unattended launch has no user, so the `user`-scope credential the `api`
  // fixture seeds does not qualify: the save is refused with a 400 until the
  // backend's credential is stored at project or global scope (`SPEC.md`,
  // "Agent profiles"). Obviously fake, and the stub image authenticates
  // nothing with it (rule 3).
  credentials.push({
    client: api,
    id: (
      await setProjectSecret(
        api,
        project.id,
        "CLAUDE_CODE_OAUTH_TOKEN",
        FAKE_AGENT_CREDENTIAL,
      )
    ).id,
  });

  // One attempt per task, so a finished run escalates instead of putting the
  // task back where the dispatcher would pick it up again (see the head of
  // this file).
  await api.put<Project>(`/projects/${project.id}`, { max_attempts: 1 });

  // The profile the dispatcher will run. Created over REST because what this
  // scenario is about is the one field below, not the editor; `max_concurrent`
  // is 2 so that the second half is held back by the pause and by nothing
  // else.
  const profile = await api.post<Profile>(`/projects/${project.id}/profiles`, {
    name: "auto-implementer",
    kind: "ephemeral",
    serves_states: [SERVED],
    max_concurrent: 2,
  });
  expect(profile.auto_launch).toBe(false);

  const first = await createTask(api, project.id, {
    title: "Dispatch me first",
    state: "backlog",
  });
  const second = await createTask(api, project.id, {
    title: "Wait for the resume",
    state: "backlog",
  });

  await loginViaToken(context, user);

  // ---- the user turns automation on ----
  await page.goto(`/projects/${project.id}?tab=profiles`);
  await page
    .getByRole("row")
    .filter({ has: page.getByText(profile.name, { exact: true }) })
    .getByRole("button", { name: "Edit" })
    .click();

  const editor = page.getByRole("form", { name: `Edit ${profile.name}` });
  await editor
    .getByRole("checkbox", { name: /Let the dispatcher launch this profile/ })
    .check();
  await editor.getByRole("button", { name: "Save profile" }).click();

  const stored = await waitFor(
    async () => {
      const profiles = await api.get<Profile[]>(
        `/projects/${project.id}/profiles`,
      );
      const saved = profiles.find((entry) => entry.id === profile.id);
      return saved?.auto_launch === true ? saved : null;
    },
    { description: "the profile to be saved with auto_launch" },
  );
  expect(stored.serves_states).toEqual([SERVED]);
  expect(stored.max_concurrent).toBe(2);

  // Nothing has been dispatched yet: both tasks are in `backlog`, which this
  // profile does not serve.
  expect(await listProjectSessions(api, project.id)).toHaveLength(0);

  // ---- a task moved into the served state starts a session by itself ----
  const firstPanel = await openTask(page, project, first.number);
  await moveFromDrawer(firstPanel, SERVED);

  const dispatched = await waitForDispatched(api, project, first.id);
  expect(dispatched.created_by).toBeNull();
  expect(dispatched.kind).toBe("ephemeral");
  expect(dispatched.profile_id).toBe(profile.id);
  // The session takes the task's title, so the list row below is findable by
  // it (`SPEC.md`, "Sessions").
  expect(dispatched.title).toBe(first.title);

  // On the task: the drawer links it, under `Sessions` and, while it still
  // holds the task, as the holder.
  await expect(
    firstPanel.locator(`a[href="/sessions/${dispatched.id}"]`).first(),
  ).toBeVisible();

  // In the session list: the row carries the `dispatcher` tag where a person's
  // name would be (`SPEC.md`, "Frontend" → "Launch source").
  await page.goto(`/projects/${project.id}?tab=sessions`);
  const row = page
    .getByRole("row")
    .filter({ has: page.getByRole("link", { name: first.title }) });
  await expect(row.getByTestId(LAUNCH_SOURCE)).toHaveText(/dispatcher/);

  // And in its own header.
  await page.goto(`/sessions/${dispatched.id}`);
  await expect(page.getByTestId(LAUNCH_SOURCE).first()).toHaveText(
    /dispatcher/,
  );

  // ---- the pause holds the next one ----
  await setPaused(page, project, true);
  await expect(page.getByText("automation paused")).toBeVisible();

  const secondPanel = await openTask(page, project, second.number);
  await moveFromDrawer(secondPanel, SERVED);

  // The move is the notice the dispatcher acts on — it is what started the
  // first session a moment ago — and the job runs 250 ms later. Meanwhile the
  // first session runs its prompt to the end on a real container, which is
  // several more wake-ups and a freed slot.
  await waitForSessionState(api, dispatched.id, "done", 180_000);

  // And the first task is a person's now: one claim against a `max_attempts`
  // of 1, so the release escalated it out of every state this profile serves.
  // Nothing is left for the dispatcher to pick up but the second task.
  const escalated = await escalatedTask(api, project, first.number);
  expect(escalated.lease_holder_session_id).toBeNull();
  expect(escalated.needs_human_reason).toContain("attempt limit reached (1/1)");

  const duringPause = await listProjectSessions(api, project.id);
  expect(duringPause.map((session) => session.id)).toEqual([dispatched.id]);
  const held = await getTask(api, project.id, second.number);
  expect(held.lease_holder_session_id).toBeNull();
  expect(held.attempts).toBe(0);
  expect(held.state).toBe(SERVED);

  // ---- and the resume lets it through ----
  await setPaused(page, project, false);
  await expect(page.getByText("automation paused")).toHaveCount(0);

  const resumed = await waitForDispatched(api, project, second.id);
  expect(resumed.id).not.toBe(dispatched.id);
  expect(resumed.created_by).toBeNull();
  expect(resumed.title).toBe(second.title);

  // The lease is given back by the v1 paths alone: an ephemeral session ends
  // itself at its first `result`, and the session-ended hook releases what it
  // held (`ARCHITECTURE.md`, "Task tracker" → "Liveness comes from the
  // session").
  await waitForSessionState(api, resumed.id, "done", 180_000);
  const released = await escalatedTask(api, project, second.number);
  expect(released.lease_holder_session_id).toBeNull();
  expect(released.needs_human_reason).toContain("attempt limit reached (1/1)");
  // The claim the launch made is on the record even though the escalating move
  // reset the counter behind it (`SPEC.md`, "Tasks": a state change resets
  // `attempts`).
  expect(released.sessions.map((touch) => touch.session_id)).toContain(
    resumed.id,
  );
});
