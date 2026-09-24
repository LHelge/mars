// "Automatic merge" and "Round limit" from the user's side (`SPEC.md`,
// "User-facing features"; "Task states": `auto_merge` and `conflict_state`;
// "Tasks": `rounds` and `max_rounds`; "Frontend", "Task board": the column's
// `auto-merge` chip, the card's `round n/max`, the states editor's toggle and
// the settings form's `Max rounds`; `ARCHITECTURE.md`, "Task tracker" →
// "Automatic merges" and "Rounds"; ADR 0045, 0046).
//
// One sentence per scenario: an approved hand-off that arrives in a new
// project's `merge` column is merged by the orchestrator with no session and
// closes the task, a conflicting one comes back to `ready` with its paths and
// leaves `main` alone, turning the toggle off in the states editor leaves the
// next approved task waiting in `merge`, and a conflict at the round limit
// escalates the task to `needs_human` with the reason.
//
// **Where the work comes from** is `handoffs.spec.ts`'s answer: the stub
// executes nothing, so a scenario commits into the implementer session's work
// clone from the host and publishes the revision over REST. Every hand-off and
// review here is arrangement — the drawer's forms are `handoffs.spec.ts` — and
// what the browser watches is what the orchestrator does by itself afterwards,
// arriving through the task stream's refresh.
//
// **Where a conflict comes from.** Every implementer is launched from `main`
// and rewrites `src/app.txt`; a conflict scenario then moves `main` under it
// with a different rewrite of the same file, through upstream, which is how
// `git.spec.ts` makes its own conflict.
//
// **The round limit is reached by the system, not by a session.** The
// escalation applies to a send-back by a session or by the system; a session's
// `changes_requested` forward would have to come from inside a stub container
// over MCP, which replays a transcript and calls nothing. The auto-merge job's
// conflict send-back is the system's, under the same rule and the same
// reason, so it is what reaches the limit here. `max_rounds` is 2 rather than
// 1 so the card has a round to show: `round n/max` appears once there has been
// more than one.
//
// **Proving that nothing happened.** "The next approved task stays in `merge`"
// is an absence, and nothing here sleeps to see one. The scenario gives the
// job something it must do instead — a task arriving in a second auto-merge
// state with no hand-off, which the job escalates — and asserts the absence
// once that has happened: the run that escalated it read its candidates after
// the approved task was already sitting in `merge`, and handles tasks by
// number, so the earlier task would have been merged first.

import type { Locator, Page } from "@playwright/test";

import { boardPath, taskPath } from "../src/tasks/taskLink";
import type { Project, TaskDetail } from "../src/types";
import type { SessionTracker } from "./utils/fixtures";
import { expect, test } from "./utils/fixtures";
import {
  commitInSessionWorkClone,
  createTask,
  forwardHandoff,
  getTask,
  gitIsAncestor,
  gitRevParse,
  listProjectSessions,
  loginViaToken,
  mirrorPath,
  moveUpstreamInto,
  publishRevision,
  taskCardTestId,
  taskColumnTestId,
  waitFor,
  waitForSessionState,
  type Api,
} from "./utils/test-helpers";

// The upstream every scenario here clones: the file the commits rewrite.
test.use({ repoFiles: { "src/app.txt": "v1\n" } });

// A container start, a clone, a hand-off publication that syncs, and a merge
// the job makes on its own.
test.setTimeout(240_000);

/**
 * A live board refresh is coalesced, and the job is woken by the task event of
 * the approval, so a merge normally lands within a second of it; the budget is
 * for a loaded machine.
 */
const LIVE_TIMEOUT = 20_000;

/** The sentence the job's conflict send-back starts with (`ARCHITECTURE.md`). */
const CONFLICT_COMMENT =
  "Merge into main conflicted; bring the branch up to date and hand off a new revision. Conflicting paths:";

// --- arrangement ------------------------------------------------------------

/**
 * A task `number` in `ready` claimed by an implementer session launched for it
 * from `main`, waited to `running` — which is what says its work clone exists.
 * Returns the session id.
 */
async function implementer(
  sessions: SessionTracker,
  client: Api,
  project: Project,
  number: number,
): Promise<string> {
  const task = await createTask(client, project.id, {
    title: `Change the app (${String(number)})`,
    state: "ready",
  });
  expect(task.number).toBe(number);
  const session = await sessions.launch(client, project.id, {
    task_id: number,
    base_ref: "main",
  });
  await waitForSessionState(client, session.id, "running", 120_000);
  return session.id;
}

/**
 * Publishes the session's tip as a revision into `review` and approves it into
 * `merge`, as a reviewer would: the move the job reacts to.
 */
async function handOffAndApprove(
  client: Api,
  project: Project,
  number: number,
  source: string,
  commit: string,
): Promise<void> {
  const revision = await publishRevision(client, project.id, number, {
    source,
    commit,
    comment: "Ready for review",
    state: "review",
  });
  await forwardHandoff(client, project.id, number, {
    handoffId: revision.id,
    comment: "LGTM",
    state: "merge",
    review: "approved",
  });
}

/** Waits until task `number` is in `state` and returns it. */
function waitForTaskState(
  client: Api,
  project: Project,
  number: number,
  state: string,
): Promise<TaskDetail> {
  return waitFor(
    async () => {
      const task = await getTask(client, project.id, number);
      return task.state === state ? task : null;
    },
    {
      timeoutMs: LIVE_TIMEOUT,
      description: `task #${String(number)} to reach ${state}`,
    },
  );
}

// --- the board --------------------------------------------------------------

async function openBoard(page: Page, project: Project): Promise<void> {
  await page.goto(boardPath(project.id, new URLSearchParams()));
  await expect(page.getByTestId(taskColumnTestId("backlog"))).toBeVisible();
}

function column(page: Page, name: string): Locator {
  return page.getByTestId(taskColumnTestId(name));
}

function card(page: Page, number: number): Locator {
  return page.getByTestId(taskCardTestId(number));
}

/** The column header's chip that says the column empties by itself. */
function autoMergeChip(page: Page, name: string): Locator {
  return column(page, name).getByText("auto-merge", { exact: true });
}

/** Opens a task's drawer by its own route. */
async function openTask(
  page: Page,
  project: Project,
  number: number,
): Promise<Locator> {
  await page.goto(taskPath(project.id, number));
  const panel = page.getByRole("dialog");
  await expect(panel).toHaveAttribute("aria-label", `Task #${String(number)}`);
  return panel;
}

/**
 * A row of the states editor, by its name cell: an auto-merge row's conflict
 * select lists the other queue states, so a row's text alone matches several.
 */
function stateRow(page: Page, name: string): Locator {
  return page
    .getByRole("row")
    .filter({ has: page.getByRole("cell", { name, exact: true }) });
}

// --- scenarios --------------------------------------------------------------

test("an approved hand-off in merge is merged without a session and closes the task", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const mirror = mirrorPath(project.id);
  const session = await implementer(sessions, api, project, 1);
  const commit = commitInSessionWorkClone(
    session,
    { "src/app.txt": "v2\n" },
    "feat: v2",
  );
  const before = gitRevParse(mirror, "main");

  // A new project's `merge` is an auto-merge state, and the board says so on
  // that column alone (`SPEC.md`, "Task states": every other default state has
  // `auto_merge: false`).
  await openBoard(page, project);
  await expect(autoMergeChip(page, "merge")).toBeVisible();
  for (const other of ["backlog", "ready", "review", "needs_human", "done"]) {
    await expect(autoMergeChip(page, other)).toHaveCount(0);
  }

  await handOffAndApprove(api, project, 1, session, commit);

  // The card leaves `merge` for `done` without anyone touching it, and the
  // board follows through the task stream.
  await expect(column(page, "done").getByTestId(taskCardTestId(1))).toBeVisible(
    { timeout: LIVE_TIMEOUT },
  );
  await expect(
    column(page, "merge").getByTestId(taskCardTestId(1)),
  ).toHaveCount(0);

  // The thread names the commit it merged: the pinned one.
  const panel = await openTask(page, project, 1);
  await expect(panel.getByText(`Merged ${commit} into main.`)).toBeVisible();

  // The integration head moved to include it — a fast-forward, since `main`
  // had not moved since the session's base.
  const after = gitRevParse(mirror, "main");
  expect(after).not.toBe(before);
  expect(gitIsAncestor(mirror, commit, "main")).toBe(true);

  const task = await getTask(api, project.id, 1);
  expect(task.state).toBe("done");
  expect(task.closed_at).not.toBeNull();
  expect(task.handoff?.review_status).toBe("approved");
  // No agent, no session: the implementer is the project's only one.
  const launched = await listProjectSessions(api, project.id);
  expect(launched.map((one) => one.id)).toEqual([session]);
});

test("a conflicting hand-off comes back to ready with its paths and main is unchanged", async ({
  page,
  context,
  user,
  api,
  repo,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const mirror = mirrorPath(project.id);
  const session = await implementer(sessions, api, project, 1);
  // `main` moves under the session with a different rewrite of the same file.
  await moveUpstreamInto(
    api,
    project,
    repo,
    { "src/app.txt": "upstream\n" },
    "Rewrite the file upstream",
  );
  const commit = commitInSessionWorkClone(
    session,
    { "src/app.txt": "session\n" },
    "feat: rewrite the file in the session",
  );
  const before = gitRevParse(mirror, "main");

  await openBoard(page, project);
  await handOffAndApprove(api, project, 1, session, commit);

  // Back to the conflict state, `ready`, by itself (`SPEC.md`, "Task states": a
  // new project's `merge` has `conflict_state: "ready"`).
  const sent = await waitForTaskState(api, project, 1, "ready");
  await expect(
    column(page, "ready").getByTestId(taskCardTestId(1)),
  ).toBeVisible({ timeout: LIVE_TIMEOUT });

  // The thread says why and which paths, for the implementer who takes it up.
  const panel = await openTask(page, project, 1);
  const comment = panel
    .getByRole("listitem")
    .filter({ hasText: "Merge into main conflicted" });
  await expect(comment).toBeVisible();
  await expect(comment).toContainText(CONFLICT_COMMENT);
  await expect(comment).toContainText("src/app.txt");

  // A conflict is a stop, not a partial write.
  expect(gitRevParse(mirror, "main")).toBe(before);
  expect(gitIsAncestor(mirror, commit, "main")).toBe(false);
  // The approved hand-off stays current, so the next implementer starts from it.
  expect(sent.handoff?.commit).toBe(commit);
  expect(sent.handoff?.review_status).toBe("approved");
  expect(sent.closed_at).toBeNull();
});

test("turning auto-merge off in the states editor leaves the next approved task in merge", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const mirror = mirrorPath(project.id);

  // The editor shows `merge` as the project was seeded: on, conflicts to
  // `ready`.
  await page.goto(`/projects/${project.id}?tab=states`);
  const merge = stateRow(page, "merge");
  const toggle = merge.getByRole("checkbox", { name: "Auto-merge" });
  await expect(toggle).toBeChecked();
  await expect(merge.getByLabel("Conflict state")).toHaveValue("ready");
  // A state that is not a queue has no toggle at all.
  await expect(
    stateRow(page, "done").getByRole("checkbox", { name: "Auto-merge" }),
  ).toHaveCount(0);

  // Off, and saved: turning it off takes the conflict state with it.
  await toggle.uncheck();
  await expect(merge.getByLabel("Conflict state")).toHaveCount(0);
  await merge.getByRole("button", { name: "Save" }).click();
  await expect(merge.getByRole("button", { name: "Save" })).toHaveCount(0);
  await expect(toggle).not.toBeChecked();

  const states = await api.get<
    { name: string; auto_merge: boolean; conflict_state: string | null }[]
  >(`/projects/${project.id}/task-states`);
  const stored = states.find((one) => one.name === "merge");
  expect(stored?.auto_merge).toBe(false);
  expect(stored?.conflict_state).toBeNull();

  // The board's column has lost its chip.
  await openBoard(page, project);
  await expect(column(page, "merge")).toBeVisible();
  await expect(autoMergeChip(page, "merge")).toHaveCount(0);

  // The barrier (see the header): a second auto-merge state, arranged over
  // REST, for the job to prove it ran.
  await api.post(`/projects/${project.id}/task-states`, {
    name: "ship",
    kind: "queue",
    auto_merge: true,
    conflict_state: "ready",
  });

  const session = await implementer(sessions, api, project, 1);
  const commit = commitInSessionWorkClone(
    session,
    { "src/app.txt": "v2\n" },
    "feat: v2",
  );
  const before = gitRevParse(mirror, "main");
  await handOffAndApprove(api, project, 1, session, commit);
  await expect(
    column(page, "merge").getByTestId(taskCardTestId(1)),
  ).toBeVisible({ timeout: LIVE_TIMEOUT });

  // A task with nothing approved arriving in `ship` is escalated by the job.
  await createTask(api, project.id, {
    title: "Nothing to merge",
    state: "ship",
  });
  const escalated = await waitForTaskState(api, project, 2, "needs_human");
  expect(escalated.needs_human_reason).toBe(
    "ship is an auto-merge state and the task has no approved hand-off",
  );

  // That run found the approved task in `merge` and left it there.
  const waiting = await getTask(api, project.id, 1);
  expect(waiting.state).toBe("merge");
  expect(waiting.handoff?.review_status).toBe("approved");
  expect(gitRevParse(mirror, "main")).toBe(before);
  await expect(
    column(page, "merge").getByTestId(taskCardTestId(1)),
  ).toBeVisible();
  await expect(
    column(page, "needs_human").getByTestId(taskCardTestId(2)),
  ).toBeVisible({ timeout: LIVE_TIMEOUT });
});

test("a conflicting merge at the round limit escalates to needs_human with the reason", async ({
  page,
  context,
  user,
  api,
  repo,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const mirror = mirrorPath(project.id);

  // `Max rounds` beside `Max attempts` in the project settings.
  await page.goto(`/projects/${project.id}`);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const settings = page.getByRole("form", { name: "Project settings" });
  await expect(settings.getByLabel("Max rounds")).toHaveValue("5");
  await settings.getByLabel("Max rounds").fill("2");
  await settings.getByRole("button", { name: "Save settings" }).click();
  await expect(settings.getByText("Settings saved.")).toBeVisible();
  const saved = await api.get<Project>(`/projects/${project.id}`);
  expect(saved.max_rounds).toBe(2);

  const session = await implementer(sessions, api, project, 1);
  await moveUpstreamInto(
    api,
    project,
    repo,
    { "src/app.txt": "upstream\n" },
    "Rewrite the file upstream",
  );

  // Round 1, sent back by a person: a person's send-back is never redirected,
  // and it is below the limit besides.
  const first = commitInSessionWorkClone(
    session,
    { "src/app.txt": "session\n" },
    "feat: rewrite the file",
  );
  const revision = await publishRevision(api, project.id, 1, {
    source: session,
    commit: first,
    comment: "Ready for review",
    state: "review",
  });
  await forwardHandoff(api, project.id, 1, {
    handoffId: revision.id,
    comment: "Please rename",
    state: "ready",
    review: "changes_requested",
  });

  // Round 2: the card now shows its round against the project's limit.
  const second = commitInSessionWorkClone(
    session,
    { "src/app.txt": "session, renamed\n" },
    "fix: rename",
  );
  await openBoard(page, project);
  const again = await publishRevision(api, project.id, 1, {
    source: session,
    commit: second,
    comment: "Renamed as asked",
    state: "review",
  });
  await expect(
    column(page, "review").getByTestId(taskCardTestId(1)),
  ).toBeVisible({ timeout: LIVE_TIMEOUT });
  await expect(card(page, 1)).toContainText("round 2/2");

  // Approved into `merge`, where the job's merge conflicts: the send-back that
  // would start a third round goes to the human state instead.
  const before = gitRevParse(mirror, "main");
  await forwardHandoff(api, project.id, 1, {
    handoffId: again.id,
    comment: "LGTM",
    state: "merge",
    review: "approved",
  });

  const escalated = await waitForTaskState(api, project, 1, "needs_human");
  expect(escalated.needs_human_reason).toBe(
    `round limit reached (2/2): ${CONFLICT_COMMENT}\nsrc/app.txt`,
  );
  expect(escalated.rounds).toBe(2);
  expect(escalated.handoff?.review_status).toBe("approved");
  expect(gitRevParse(mirror, "main")).toBe(before);

  // The card lands in `needs_human` with the reason and its round on it.
  const escalatedCard = column(page, "needs_human").getByTestId(
    taskCardTestId(1),
  );
  await expect(escalatedCard).toBeVisible({ timeout: LIVE_TIMEOUT });
  await expect(escalatedCard).toContainText("round limit reached (2/2)");
  await expect(escalatedCard).toContainText("round 2/2");
});
