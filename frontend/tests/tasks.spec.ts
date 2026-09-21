// The task board against a real orchestrator (`SPEC.md`, "User-facing
// features", "Task board"; "Tasks"; "Task states"; "Frontend", "Task board",
// "Task-board search", "Board refresh ordering" and "Copy links").
//
// Everything a session does to a task lives in the session specs. What is here
// is the board a user drives on their own: the columns the project's states
// make, creating, editing and commenting on a card, moving it between columns
// from the drawer, the `blocks` edges and the blocked indicator, parent/child
// closure, the search field, the deep link and its `Copy link`, the states
// editor with its refusals, and — the one that proves ADR 0022 end to end — a
// second browser context that follows the first without ever reloading.
//
// Arrangement is through the helpers (`tests/utils/`) and therefore through
// the REST API: a scenario that is about moving a card does not spend its first
// thirty seconds creating one through a form. What each scenario asserts
// through the browser is only what it is about.

import type { Locator, Page } from "@playwright/test";

import type { Project } from "../src/types";
import { expect, test } from "./utils/fixtures";
import {
  baseUrl,
  commentOnTask,
  createBareRepo,
  createProject,
  createTask,
  createTestUser,
  getTask,
  loginViaToken,
  moveTask,
  newLoggedInPage,
  TASK_COLUMN_PREFIX,
  taskCardTestId,
  taskColumnTestId,
  uniqueName,
} from "./utils/test-helpers";

/** `docs/data-model.md`, `task_states`: what every project is created with. */
const DEFAULT_STATES = [
  "backlog",
  "ready",
  "review",
  "merge",
  "needs_human",
  "done",
  "cancelled",
];

/** A live refresh is coalesced, so nothing here asserts immediacy (ADR 0022). */
const LIVE_TIMEOUT = 10_000;

function boardPath(project: Project): string {
  return `/projects/${project.id}?tab=board`;
}

/** The board tab, waited for on the first column the project is created with. */
async function openBoard(page: Page, project: Project): Promise<void> {
  await page.goto(boardPath(project));
  await expect(page.getByTestId(taskColumnTestId("backlog"))).toBeVisible();
}

function column(page: Page, name: string): Locator {
  return page.getByTestId(taskColumnTestId(name));
}

function card(page: Page, number: number): Locator {
  return page.getByTestId(taskCardTestId(number));
}

/** The task drawer of `/projects/:id/tasks/:number`. */
function drawer(page: Page): Locator {
  return page.getByRole("dialog");
}

/** Opens a card's drawer by clicking the card's own link. */
async function openCard(page: Page, number: number): Promise<Locator> {
  await card(page, number).getByRole("link").first().click();
  const panel = drawer(page);
  await expect(panel).toHaveAttribute("aria-label", `Task #${String(number)}`);
  return panel;
}

/** The board's column names, in the order they are rendered. */
function columnNames(page: Page): Promise<string[]> {
  return page
    .locator(`[data-testid^='${TASK_COLUMN_PREFIX}']`)
    .evaluateAll(
      (nodes, prefix: string) =>
        nodes.map((node) =>
          (node.getAttribute("data-testid") ?? "").slice(prefix.length),
        ),
      TASK_COLUMN_PREFIX,
    );
}

/** Moves the open task through the drawer's `Move to` control. */
async function moveFromDrawer(panel: Locator, to: string): Promise<void> {
  await panel.getByLabel("Move to").selectOption(to);
  await panel.getByRole("button", { name: "Move", exact: true }).click();
  await panel.getByRole("button", { name: `Move to ${to}` }).click();
}

test("columns show the default states in order and an empty project invites a first task", async ({
  page,
  context,
  user,
  project,
}) => {
  await loginViaToken(context, user);

  await openBoard(page, project);

  expect(await columnNames(page)).toEqual(DEFAULT_STATES);

  // The empty board is an invitation, not a failed search (ADR 0031).
  await expect(page.getByText("No tasks yet")).toBeVisible();
  await expect(page.getByText("No matching tasks")).toHaveCount(0);
});

test("a task created from the board form lands in backlog and opens in the drawer", async ({
  page,
  context,
  user,
  project,
}) => {
  await loginViaToken(context, user);
  await openBoard(page, project);

  await page.getByRole("button", { name: "New task" }).first().click();
  const form = page.getByRole("form", { name: "New task" });
  await form.getByLabel("Title").fill("Write docs");
  await form.getByLabel("Priority").selectOption("1");
  await form.getByLabel("Labels").fill("docs");
  await form.getByRole("button", { name: "Create task" }).click();

  // `POST` answers 201 in the project's first queue state (`SPEC.md`, "Tasks").
  const first = column(page, "backlog").getByTestId(taskCardTestId(1));
  await expect(first).toBeVisible();
  await expect(first).toContainText("Write docs");
  await expect(first).toContainText("P1");
  await expect(first).toContainText("docs");

  const panel = await openCard(page, 1);
  await expect(page).toHaveURL(new RegExp(`/projects/${project.id}/tasks/1$`));
  await expect(panel.getByText("No description")).toBeVisible();
  await expect(panel.getByText("No comments yet.")).toBeVisible();
  await expect(panel.getByText("No dependencies.")).toBeVisible();
});

test("an edit and a comment made in the drawer survive a reload", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await createTask(api, project.id, { title: "Draft the plan" });
  await loginViaToken(context, user);
  await openBoard(page, project);

  const panel = await openCard(page, 1);

  await panel.getByRole("button", { name: "Edit" }).click();
  const form = panel.getByRole("form", { name: "Edit task #1" });
  await form.getByLabel("Title").fill("Draft the migration plan");
  await form.getByLabel("Description").fill("Two steps, both reversible.");
  await form.getByRole("button", { name: "Save changes" }).click();

  await expect(
    panel.getByText("Two steps, both reversible."),
  ).toBeVisible();

  await panel.getByLabel("Add a comment").fill("Starting on this tomorrow.");
  await panel.getByRole("button", { name: "Comment" }).click();
  await expect(panel.getByText("Starting on this tomorrow.")).toBeVisible();

  await page.reload();
  const reopened = drawer(page);
  await expect(
    reopened.getByRole("heading", { name: "Draft the migration plan" }),
  ).toBeVisible();
  await expect(reopened.getByText("Two steps, both reversible.")).toBeVisible();
  await expect(reopened.getByText("Starting on this tomorrow.")).toBeVisible();
  // Nothing here is the tracker's own doing, so no system comment exists yet.
  await expect(
    reopened.getByTitle("Written by the tracker itself"),
  ).toHaveCount(0);
});

test("an edit open on one task is discarded when the drawer shows a cached other one", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  // The child link is how A reaches B without typing a URL, and B is read once
  // first so the second visit is served from the query cache — no loading
  // state in between, which is the whole hazard (`SPEC.md`, "Frontend",
  // "Task board").
  const parent = await createTask(api, project.id, { title: "Alpha plan" });
  await createTask(api, project.id, {
    title: "Bravo plan",
    parent_id: parent.id,
  });
  await loginViaToken(context, user);
  await openBoard(page, project);

  await openCard(page, 2);
  await page.goBack();
  const alpha = await openCard(page, 1);

  await alpha.getByRole("button", { name: "Edit" }).click();
  const alphaForm = alpha.getByRole("form", { name: "Edit task #1" });
  await alphaForm.getByLabel("Title").fill("Alpha draft that must not travel");

  await alpha.getByRole("link", { name: /Bravo plan/ }).click();

  const bravo = drawer(page);
  await expect(bravo).toHaveAttribute("aria-label", "Task #2");
  // The edit belonged to #1: nothing is open on #2, and no field of #2 holds
  // #1's words.
  await expect(bravo.getByRole("form", { name: /^Edit task/ })).toHaveCount(0);
  await expect(bravo.getByText("Alpha draft that must not travel")).toHaveCount(
    0,
  );

  // Opening #2's own form starts from #2, so #1's draft cannot be saved here.
  await bravo.getByRole("button", { name: "Edit" }).click();
  const bravoForm = bravo.getByRole("form", { name: "Edit task #2" });
  await expect(bravoForm.getByLabel("Title")).toHaveValue("Bravo plan");
  await bravoForm.getByRole("button", { name: "Save changes" }).click();

  expect((await getTask(api, project.id, 1)).title).toBe("Alpha plan");
  expect((await getTask(api, project.id, 2)).title).toBe("Bravo plan");
});

test("a draft in the drawer survives a refresh of the same task", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await createTask(api, project.id, { title: "Alpha plan" });
  await loginViaToken(context, user);
  await openBoard(page, project);

  const panel = await openCard(page, 1);
  await panel.getByRole("button", { name: "Edit" }).click();
  const form = panel.getByRole("form", { name: "Edit task #1" });
  await form.getByLabel("Title").fill("Alpha plan, revised");

  // A `commented` event invalidates the open detail, so the drawer refetches
  // the very task being edited: the same resource, and therefore the same
  // draft.
  await commentOnTask(api, project.id, 1, "Landed from another client.");
  await expect(panel.getByText("Landed from another client.")).toBeVisible({
    timeout: LIVE_TIMEOUT,
  });

  await expect(form.getByLabel("Title")).toHaveValue("Alpha plan, revised");
  await form.getByRole("button", { name: "Save changes" }).click();
  await expect(
    panel.getByRole("heading", { name: "Alpha plan, revised" }),
  ).toBeVisible();
  expect((await getTask(api, project.id, 1)).title).toBe("Alpha plan, revised");
});

test("the drawer moves a card across columns and closes and reopens it", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await createTask(api, project.id, { title: "Ship the thing" });
  await loginViaToken(context, user);
  await openBoard(page, project);

  const panel = await openCard(page, 1);

  await moveFromDrawer(panel, "ready");
  await expect(column(page, "ready").getByTestId(taskCardTestId(1))).toBeVisible();
  await expect(panel.getByText("Closed", { exact: true })).toHaveCount(0);

  // A terminal state sets `closed_at`, which the drawer shows as `Closed`.
  await moveFromDrawer(panel, "done");
  await expect(column(page, "done").getByTestId(taskCardTestId(1))).toBeVisible();
  await expect(panel.getByText("Closed", { exact: true })).toBeVisible();

  await moveFromDrawer(panel, "backlog");
  await expect(
    column(page, "backlog").getByTestId(taskCardTestId(1)),
  ).toBeVisible();
  await expect(panel.getByText("Closed", { exact: true })).toHaveCount(0);
});

test("a blocks dependency blocks a card, clears when it closes, and refuses a cycle", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await createTask(api, project.id, { title: "Alpha" });
  await createTask(api, project.id, { title: "Beta" });
  await loginViaToken(context, user);
  await openBoard(page, project);

  // Beta blocks on Alpha, added from Beta's own drawer.
  const beta = await openCard(page, 2);
  const add = beta.getByRole("form", { name: "Add dependency" });
  await add.getByLabel("Find a task").fill("#1");
  await add.getByLabel("Task", { exact: true }).selectOption({ label: "#1 Alpha" });
  await add.getByRole("button", { name: "Add dependency" }).click();

  await expect(beta.getByRole("heading", { name: "Blocks on" })).toBeVisible();
  await expect(card(page, 2)).toContainText("blocked");
  await expect(card(page, 2)).toContainText("↑1");

  // Closing Alpha from outside the browser unblocks Beta through the stream.
  await moveTask(api, project.id, 1, "done");
  await expect(card(page, 2)).not.toContainText("blocked", {
    timeout: LIVE_TIMEOUT,
  });

  // The other direction would close a cycle: 409, and no edge is added.
  await page.goto(`/projects/${project.id}/tasks/1`);
  const alpha = drawer(page);
  await expect(alpha).toHaveAttribute("aria-label", "Task #1");
  const addOnAlpha = alpha.getByRole("form", { name: "Add dependency" });
  await addOnAlpha.getByLabel("Find a task").fill("#2");
  await addOnAlpha
    .getByLabel("Task", { exact: true })
    .selectOption({ label: "#2 Beta" });
  await addOnAlpha.getByRole("button", { name: "Add dependency" }).click();

  await expect(alpha.getByText(/cycle/)).toBeVisible();
  await expect(alpha.getByRole("heading", { name: "Blocks on" })).toHaveCount(0);
  const detail = await getTask(api, project.id, 1);
  expect(detail.depends_on).toEqual([]);
});

test("a parent closes by itself when its last child closes", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  const parent = await createTask(api, project.id, { title: "The epic" });
  await createTask(api, project.id, {
    title: "First child",
    parent_id: parent.id,
  });
  await createTask(api, project.id, {
    title: "Second child",
    parent_id: parent.id,
  });
  await loginViaToken(context, user);
  await openBoard(page, project);

  await expect(card(page, 1)).toContainText("blocked");

  const panel = await openCard(page, 1);
  await expect(panel.getByRole("link", { name: /First child/ })).toBeVisible();
  await expect(panel.getByRole("link", { name: /Second child/ })).toBeVisible();

  await moveTask(api, project.id, 2, "done");
  await moveTask(api, project.id, 3, "done");

  // `ARCHITECTURE.md`, "Task tracker": the last closing child closes the
  // parent, as a `state_changed` carrying actor `system`. The tracker writes
  // no comment for it, so the drawer has none to render.
  await expect(column(page, "done").getByTestId(taskCardTestId(1))).toBeVisible({
    timeout: LIVE_TIMEOUT,
  });
  await expect(panel.getByText("Closed", { exact: true })).toBeVisible({
    timeout: LIVE_TIMEOUT,
  });
  expect((await getTask(api, project.id, 1)).state).toBe("done");
});

test("board search matches titles and exact numbers, and resets on a project change", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await createTask(api, project.id, { title: "Fix Login Redirect" });
  await createTask(api, project.id, { title: "login page copy" });
  await createTask(api, project.id, { title: "Other" });
  // Up to #12, so `#2` has a `#12` to not match.
  for (let n = 4; n <= 12; n += 1) {
    await createTask(api, project.id, { title: `Filler ${String(n)}` });
  }

  const second = createBareRepo("search-second");
  const other = await createProject(api, {
    name: uniqueName("search-other"),
    remote_url: second.url,
  });

  await loginViaToken(context, user);
  await openBoard(page, project);
  await expect(card(page, 12)).toBeVisible();

  const search = page.getByLabel("Search tasks");
  await expect(search).toHaveAttribute("placeholder", "Search title or #number");

  // A case-insensitive substring of the title.
  await search.fill("login");
  await expect(card(page, 1)).toBeVisible();
  await expect(card(page, 2)).toBeVisible();
  await expect(card(page, 3)).toHaveCount(0);
  await expect(card(page, 12)).toHaveCount(0);

  // A number, with or without the `#`, is that one task and no prefix of it.
  for (const query of ["#2", "2"]) {
    await search.fill(query);
    await expect(card(page, 2)).toBeVisible();
    await expect(card(page, 1)).toHaveCount(0);
    await expect(card(page, 12)).toHaveCount(0);
  }

  await search.fill("xyz");
  await expect(page.getByText("No matching tasks")).toBeVisible();
  await expect(column(page, "backlog")).toBeVisible();

  await page.getByRole("button", { name: "Clear search" }).first().click();
  await expect(card(page, 12)).toBeVisible();

  // A refresh reapplies the query to the new snapshot rather than dropping it.
  await search.fill("login");
  await createTask(api, project.id, { title: "login smoke test" });
  await expect(card(page, 13)).toBeVisible({ timeout: LIVE_TIMEOUT });
  await expect(search).toHaveValue("login");
  await expect(card(page, 3)).toHaveCount(0);

  // Another project is another board, and another search.
  await page.goto(boardPath(other));
  await expect(page.getByLabel("Search tasks")).toHaveValue("");
});

test("a task link opens the drawer directly and Copy link writes the canonical URL", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await createTask(api, project.id, { title: "First" });
  await createTask(api, project.id, { title: "Second" });
  await loginViaToken(context, user);
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);

  await page.goto(`/projects/${project.id}/tasks/2`);

  const panel = drawer(page);
  await expect(panel).toHaveAttribute("aria-label", "Task #2");
  await expect(panel.getByRole("heading", { name: "Second" })).toBeVisible();
  // The board is behind the drawer, not replaced by it.
  await expect(card(page, 1)).toBeVisible();

  await panel.getByRole("button", { name: "Copy link" }).click();
  await expect(panel.getByRole("button", { name: "Link copied" })).toBeVisible();

  const copied = await page.evaluate(() => navigator.clipboard.readText());
  expect(copied).toBe(`${baseUrl()}/projects/${project.id}/tasks/2`);
});

test("the states editor adds, renames and removes a column, and says why it cannot", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await loginViaToken(context, user);

  // The editor confirms a removal with `window.confirm`.
  page.on("dialog", (dialog) => {
    void dialog.accept();
  });

  await page.goto(`/projects/${project.id}?tab=states`);
  const add = page.getByRole("form", { name: "Add a task state" });
  await expect(add).toBeVisible();

  // `qa` after `review`, which sits at position 2.
  await add.getByLabel("Name").fill("qa");
  await add.getByLabel("Kind").selectOption("queue");
  await add.getByLabel("Position").fill("3");
  await add.getByRole("button", { name: "Add state" }).click();

  const row = (name: string) => page.getByRole("row").filter({ hasText: name });
  await expect(row("qa")).toBeVisible();

  await openBoard(page, project);
  expect(await columnNames(page)).toEqual([
    "backlog",
    "ready",
    "review",
    "qa",
    "merge",
    "needs_human",
    "done",
    "cancelled",
  ]);

  // A task in the column, so the rename has something to carry with it.
  await createTask(api, project.id, { title: "Under test", state: "qa" });
  await expect(column(page, "qa").getByTestId(taskCardTestId(1))).toBeVisible({
    timeout: LIVE_TIMEOUT,
  });

  await page.goto(`/projects/${project.id}?tab=states`);
  await row("qa").getByRole("button", { name: "Rename" }).click();
  const rename = page.getByRole("form", { name: "Rename qa" });
  await rename.getByLabel("New name").fill("verify");
  await rename.getByRole("button", { name: "Save" }).click();
  await expect(row("verify")).toBeVisible();

  await openBoard(page, project);
  await expect(column(page, "verify").getByTestId(taskCardTestId(1))).toBeVisible();
  await expect(column(page, "qa")).toHaveCount(0);

  // The three refusals `SPEC.md` answers 409 with, each shown beside a
  // disabled Remove before the request rather than after it
  // (`taskStateRules.ts`; the server stays the authority).
  await page.goto(`/projects/${project.id}?tab=states`);
  await expect(row("needs_human")).toContainText(
    "The human state cannot be deleted",
  );
  await expect(row("verify")).toContainText("1 task is in this state");
  await expect(
    row("verify").getByRole("button", { name: "Remove" }),
  ).toBeDisabled();

  await row("cancelled").getByRole("button", { name: "Remove" }).click();
  await expect(row("cancelled")).toHaveCount(0);
  await expect(row("done")).toContainText(
    "The last terminal state cannot be deleted",
  );

  // Emptied, the column can go.
  await moveTask(api, project.id, 1, "backlog");
  await page.reload();
  await expect(
    row("verify").getByRole("button", { name: "Remove" }),
  ).toBeEnabled();
  await row("verify").getByRole("button", { name: "Remove" }).click();
  await expect(row("verify")).toHaveCount(0);

  await openBoard(page, project);
  await expect(column(page, "verify")).toHaveCount(0);
  await expect(column(page, "backlog").getByTestId(taskCardTestId(1))).toBeVisible();
});

test("a second browser context follows the first without reloading", async ({
  page,
  context,
  request,
  browser,
  user,
  api,
  project,
}) => {
  const watcher = await createTestUser(request, { prefix: "live-b" });
  // One task before either board opens, so the second context's arrival
  // already shows what the first has.
  await createTask(api, project.id, { title: "Already here" });
  await loginViaToken(context, user);
  await openBoard(page, project);

  const second = await newLoggedInPage(browser, watcher);
  await second.goto(boardPath(project));
  await expect(
    second.getByTestId(taskColumnTestId("backlog")),
  ).toBeVisible();

  // Installed after the arrival navigation: from here on, every change the
  // second context shows must have come down the stream (ADR 0022).
  let loads = 0;
  second.on("load", () => {
    loads += 1;
  });
  const url = second.url();

  // Created in the first browser, through its own form.
  await page.getByRole("button", { name: "New task" }).first().click();
  const form = page.getByRole("form", { name: "New task" });
  await form.getByLabel("Title").fill("Seen from over there");
  await form.getByRole("button", { name: "Create task" }).click();
  await expect(card(page, 2)).toBeVisible();

  await expect(column(second, "backlog").getByTestId(taskCardTestId(2))).toBeVisible(
    { timeout: LIVE_TIMEOUT },
  );
  await expect(card(second, 2)).toContainText("Seen from over there");

  // Moved in the first browser, from the drawer.
  const panel = await openCard(page, 2);
  await moveFromDrawer(panel, "ready");
  await expect(
    column(page, "ready").getByTestId(taskCardTestId(2)),
  ).toBeVisible();
  await expect(
    column(second, "ready").getByTestId(taskCardTestId(2)),
  ).toBeVisible({ timeout: LIVE_TIMEOUT });

  // A state renamed: `states_changed` refreshes the columns themselves.
  await page.goto(`/projects/${project.id}?tab=states`);
  const row = page.getByRole("row").filter({ hasText: "ready" });
  await row.getByRole("button", { name: "Rename" }).click();
  const rename = page.getByRole("form", { name: "Rename ready" });
  await rename.getByLabel("New name").fill("todo");
  await rename.getByRole("button", { name: "Save" }).click();

  await expect(column(second, "todo")).toBeVisible({ timeout: LIVE_TIMEOUT });
  await expect(column(second, "ready")).toHaveCount(0);

  // Deleted: the card goes with it.
  await api.delete(`/projects/${project.id}/tasks/2`);
  await expect(card(second, 2)).toHaveCount(0, { timeout: LIVE_TIMEOUT });
  await expect(card(second, 1)).toBeVisible();

  expect(loads).toBe(0);
  expect(second.url()).toBe(url);

  await second.context().close();
});

test("release is disabled while no session holds the task", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await createTask(api, project.id, { title: "Nobody has this" });
  await loginViaToken(context, user);
  await openBoard(page, project);

  const panel = await openCard(page, 1);

  await expect(panel.getByRole("button", { name: "Release" })).toBeDisabled();
  await expect(panel.getByText("No session has touched this task.")).toBeVisible();
});
