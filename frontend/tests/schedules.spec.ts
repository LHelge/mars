// Scheduled agents, end to end in a browser (`SPEC.md`, "User-facing features"
// → "Scheduled agents"; "Agent profiles" → "Scheduled profiles"; "Frontend" →
// "Scheduled profiles" and "Launch source"; `ARCHITECTURE.md`, "Task tracker" →
// "Scheduled agents"; ADR 0043).
//
// The one scenario here is a schedule closing the loop with nobody in it: a
// user gives an ephemeral stub-image profile a cron expression and a prompt in
// the profile editor, the editor shows the next run the server computed, the
// tick is made due, and a session nobody launched appears in the project's
// session list under a `schedule` tag and runs to `done`. Then automation is
// paused and the same trigger launches nothing.
//
// **How a tick is made due, and why not by waiting.** The finest period a
// 5-field cron expression can express is one minute, so waiting for a real
// occurrence means waiting for a minute boundary — and nothing in this suite
// sleeps for a fixed period. The window is injectable instead: `POST
// /api/test/scheduler-tick` runs the job once with both ends of it in the body
// (`SPEC.md`, "Test-only routes"), and `runSchedulerTick` places the floor two
// minutes before the `now` it is given. Every assertion below is therefore
// about instants this file chose.
//
// **Why 29 February.** The expression has to be one the orchestrator's own
// minute-by-minute scheduler will never find due while the run is in progress,
// because a second launch nobody asked for would arrive in the middle of the
// assertions. `0 4 29 2 *` — 04:00 UTC on a leap day — cannot come due inside
// a run, so the only thing that fires this profile is the tick this file
// makes. After that first tick `last_scheduled_at` is a 2028 instant, which
// puts the real job's window floor years ahead of its own `now` and closes the
// profile to it for good.
//
// **What the pause assertion rests on.** A job that ran and launched nothing is
// not something an API can be asked about afterwards, but this route answers
// it directly: the second tick reports the counters of its own run, and a tick
// that was claimed and then refused is `skipped`, not `items`. The project
// still having exactly one session is the other half.
//
// **Clean-up.** The session here is one nobody launched, so the project is
// registered with `sessions.sweep`: at teardown its automation is paused and
// every session it has is ended, whether or not this file learnt the id
// (`tests/utils/fixtures.ts`).

import type { Locator, Page } from "@playwright/test";

import type { Profile, Project, Session } from "../src/types";
import { expect, test } from "./utils/fixtures";
import {
  FAKE_AGENT_CREDENTIAL,
  LAUNCH_SOURCE,
  openAllSessions,
  PROFILE_AUTOMATION,
  listProjectSessions,
  loginViaToken,
  runSchedulerTick,
  setProjectSecret,
  turnOffAutoLaunch,
  waitFor,
  waitForSessionState,
  type Api,
} from "./utils/test-helpers";

// A container start, a git clone and a replayed turn, as in `sessions.spec.ts`,
// with a profile editor and a settings form around them.
test.setTimeout(240_000);

/**
 * The project-scope agent credential the scenario stores, deleted after it.
 *
 * Exactly as `dispatcher.spec.ts` does, and for the same reason: `/secrets`
 * lists every project's project-scope rows for every user, so one left behind
 * puts a second `Claude subscription token` in front of `secrets.spec.ts`.
 */
const credentials: { client: Api; id: string }[] = [];

test.afterEach(async () => {
  for (const entry of credentials.splice(0, credentials.length)) {
    await entry.client.delete(`/secrets/${entry.id}`, undefined, {
      allow: [403, 404],
    });
  }
});

/** 04:00 UTC on a leap day — see the head of this file. */
const CRON = "0 4 29 2 *";

/** What every run of this profile is asked to do. */
const PROMPT = "List the files at the repository root and stop.";

/**
 * A minute after the leap-day occurrence the first tick fires, and a minute
 * after the next one — four years on — for the tick the pause refuses.
 *
 * Both are the end of their run's window and the value a fired tick writes to
 * `last_scheduled_at`, so the second has to be later than the first for the
 * second window to hold anything at all.
 */
const FIRST_TICK = new Date("2028-02-29T04:01:00Z");
const SECOND_TICK = new Date("2032-02-29T04:01:00Z");

/** The profiles tab, with the editor of `profile` open. */
async function openEditor(
  page: Page,
  project: Project,
  profile: Profile,
): Promise<Locator> {
  await page.goto(`/projects/${project.id}?tab=profiles`);
  await page
    .getByRole("row")
    .filter({ has: page.getByText(profile.name, { exact: true }) })
    .getByRole("button", { name: "Edit" })
    .click();

  const editor = page.getByRole("form", { name: `Edit ${profile.name}` });
  await expect(editor).toBeVisible();
  return editor;
}

/** Sets the project's `Pause automation` toggle through its settings form. */
async function setPaused(
  page: Page,
  project: Project,
  paused: boolean,
): Promise<void> {
  await page.goto(`/projects/${project.id}`);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const form = page.getByRole("form", { name: "Project settings" });
  await expect(form).toBeVisible();

  const toggle = form.getByRole("checkbox", { name: /Pause automation/ });
  if (paused) {
    await toggle.check();
  } else {
    await toggle.uncheck();
  }
  await form.getByRole("button", { name: "Save settings" }).click();
  await expect(form.getByText("Settings saved.")).toBeVisible();
}

/**
 * An instant the API reported, as a comparable string.
 *
 * RFC 3339 admits more than one spelling of one instant, and what a column
 * round-trips through Postgres and serde is not something a scenario should
 * assert the punctuation of. Parsed and re-normalised, the assertion is about
 * the instant.
 */
function instant(iso: string | null): string {
  return iso === null ? "none" : new Date(iso).toISOString();
}

/** The scheduled session of this project, once the tick has produced one. */
function waitForScheduled(client: Api, project: Project): Promise<Session> {
  return waitFor(
    async () => {
      const sessions = await listProjectSessions(client, project.id);
      return (
        sessions.find((session) => session.launch_source === "schedule") ?? null
      );
    },
    {
      timeoutMs: 90_000,
      intervalMs: 250,
      description: "the scheduler to launch a session",
    },
  );
}

test("a due schedule launches a session nobody asked for, and a pause stops the next tick", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  // Everything this scenario leaves behind belongs to the project, including
  // the session it never launched.
  sessions.sweep(api, project.id);

  // The credential below is one the dispatcher can use too, and a new project's
  // seeded `implementer` and `reviewer` carry `auto_launch` (ADR 0051). Nothing
  // here puts a task in `ready` or `review`, but the rule of this suite is that
  // a project holding such a credential has them off before it is stored, so
  // the only automation live here is the schedule this scenario is about
  // (`tests/README.md`, "Automation and the seeded roles").
  await turnOffAutoLaunch(api, project.id);

  // A scheduled run has no user, so the `user`-scope credential the `api`
  // fixture seeds does not qualify and the save below would be refused with a
  // 400 until this one exists (`SPEC.md`, "Agent profiles" → "Scheduled
  // profiles"). Obviously fake, and the stub image authenticates nothing with
  // it (rule 3).
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

  // The profile the schedule will run, on the stack's stub image. Created over
  // REST because what this scenario is about is the two fields the editor
  // fills in below, not the rest of the form.
  const profile = await api.post<Profile>(`/projects/${project.id}/profiles`, {
    name: "leap-day-scan",
    kind: "ephemeral",
    max_concurrent: 2,
  });
  expect(profile.schedule_cron).toBeNull();
  expect(profile.next_scheduled_at).toBeNull();

  await loginViaToken(context, user);

  // ---- the user gives the profile a schedule ----
  const editor = await openEditor(page, project, profile);
  // The checkbox owns the fieldset: the fields are disabled until it is on.
  const cronField = editor.getByLabel("Cron expression (UTC)");
  await expect(cronField).toBeDisabled();
  await editor.getByLabel(/Run this profile on a schedule/).check();
  await cronField.fill(CRON);
  await editor.getByLabel("Schedule prompt").fill(PROMPT);
  await editor.getByRole("button", { name: "Save profile" }).click();

  // The save closes the editor and the list is back, now saying what runs this
  // profile without a person (`SPEC.md`, "Frontend" → "Scheduled profiles").
  const row = page
    .getByRole("row")
    .filter({ has: page.getByText(profile.name, { exact: true }) });
  await expect(row.getByTestId(PROFILE_AUTOMATION)).toHaveText(/schedule/);

  // And the next run is on the editor, in the viewer's zone with the UTC
  // instant under it. It is the server's answer — no cron parser ships in the
  // bundle — so a value here is the expression having been understood.
  const reopened = await openEditor(page, project, profile);
  await expect(reopened.getByLabel("Cron expression (UTC)")).toHaveValue(CRON);
  await expect(reopened.getByLabel("Next run")).toContainText("UTC");

  const saved = await api.get<Profile>(
    `/projects/${project.id}/profiles/${profile.id}`,
  );
  expect(saved.schedule_prompt).toBe(PROMPT);
  expect(saved.last_scheduled_at).toBeNull();
  expect(instant(saved.next_scheduled_at)).toBe("2028-02-29T04:00:00.000Z");

  // Nothing has run yet: the expression is years away and the orchestrator's
  // own scheduler has no window that reaches it.
  expect(await listProjectSessions(api, project.id)).toHaveLength(0);

  // ---- the tick comes due, and a session starts by itself ----
  const fired = await runSchedulerTick(api, { now: FIRST_TICK });
  // The job sweeps every `ready` project of the instance and its counters are
  // the whole sweep's, so they are read as a lower bound here and what this
  // project got is read off the project. Nothing failed, though — that counter
  // is about the sweep as a whole and a failure anywhere in it is a failure.
  expect(fired.items).toBeGreaterThanOrEqual(1);
  expect(fired.failures).toBe(0);

  const scheduled = await waitForScheduled(api, project);
  expect(scheduled.created_by).toBeNull();
  expect(scheduled.kind).toBe("ephemeral");
  expect(scheduled.profile_id).toBe(profile.id);
  expect(scheduled.task_id).toBeNull();
  // The title names the profile and the instant the run was decided at, which
  // is the `now` this file handed the job.
  expect(scheduled.title).toBe(
    `${profile.name} — scheduled run 2028-02-29T04:01:00Z`,
  );

  // In the session list: the row carries the `schedule` tag where a person's
  // name would be (`SPEC.md`, "Frontend" → "Launch source").
  await openAllSessions(page, project.id);
  const sessionRow = page.getByRole("row").filter({
    has: page.getByRole("link", { name: scheduled.title ?? "" }),
  });
  await expect(sessionRow.getByTestId(LAUNCH_SOURCE)).toHaveText(/schedule/);

  // And in its own header.
  await page.goto(`/sessions/${scheduled.id}`);
  await expect(page.getByTestId(LAUNCH_SOURCE).first()).toHaveText(/schedule/);

  // The run itself: a real container on the stub image, the schedule's prompt
  // as its message, ending at its first `result` as any ephemeral session does.
  await waitForSessionState(api, scheduled.id, "done", 180_000);

  // The editor now shows the tick on the other output.
  const afterRun = await openEditor(page, project, profile);
  await expect(afterRun.getByLabel("Last run")).toContainText("UTC");
  const advanced = await api.get<Profile>(
    `/projects/${project.id}/profiles/${profile.id}`,
  );
  expect(instant(advanced.last_scheduled_at)).toBe("2028-02-29T04:01:00.000Z");
  // `next_scheduled_at` is the first occurrence after *now*, not after the
  // tick that just fired (`SPEC.md`, "Scheduled profiles"), and now is years
  // before this expression's next leap day either way — so it is the same
  // instant it was before the run, which is the point: the tick this file made
  // due is not one the clock ever reached.
  expect(instant(advanced.next_scheduled_at)).toBe("2028-02-29T04:00:00.000Z");

  // ---- with automation paused the same trigger launches nothing ----
  await setPaused(page, project, true);
  await expect(page.getByText("automation paused")).toBeVisible();

  const refused = await runSchedulerTick(api, { now: SECOND_TICK });
  // Claimed and then spent: the pause is not a reason to leave the tick
  // pending, because a schedule says "run at these times" and not "run this
  // many times" (`ARCHITECTURE.md`, "Task tracker" → "Scheduled agents").
  // `skipped` is the sweep's, so it is a lower bound like `items` above; what
  // makes it this project's tick is the row below, which only a claim moves.
  expect(refused.skipped).toBeGreaterThanOrEqual(1);
  expect(refused.failures).toBe(0);

  const after = await listProjectSessions(api, project.id);
  expect(after.map((session) => session.id)).toEqual([scheduled.id]);

  const spent = await api.get<Profile>(
    `/projects/${project.id}/profiles/${profile.id}`,
  );
  expect(instant(spent.last_scheduled_at)).toBe("2032-02-29T04:01:00.000Z");
});
