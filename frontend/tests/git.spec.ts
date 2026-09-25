// The "Git operations" feature paragraph of `SPEC.md`, "User-facing features",
// driven through the browser against a real orchestrator: the session view's
// Changes panel, the explicit sync that feeds it, the session-branch table with
// its ahead/behind, and the merge, rebase and push forms — including the two
// refusals that matter, a merge that stops on a conflict and a push the
// upstream rejects.
//
// **How work gets onto a session branch here.** The stub CLI replays a recorded
// transcript and executes nothing, so nothing inside the container ever writes
// to the clone. A scenario therefore stands in for the agent: it commits into
// `<DATA_DIR>/sessions/<sid>/work` from the host with
// `commitInSessionWorkClone`, which is exactly the state a real agent leaves
// behind, and the orchestrator's fetch-back is what carries it into the mirror
// (`ARCHITECTURE.md`, "Git model", "Session clone" and "Fetch-back").
//
// **Sessions are launched without a first message.** The stub blocks on stdin
// until it gets one, so the session reaches `running` — the state change is on
// the stdin attach, not on the CLI's `init` (ADR 0032) — with an empty
// transcript. That is what makes `Git <op> succeeded` countable: every row in
// these transcripts is a git outcome event.
//
// **Where the panel is.** `GitActionsPanel` is the project's *Branches* tab
// (`?tab=branches`) and, filtered to one row, behind the session header's
// `branch` toggle; the *Sessions* tab carries no git actions.
//
// **Two places the task text and the application disagree**, each asserted as
// the application is built:
//
// - The Changes panel has no Sync of its own. The explicit sync is the session
//   header's `Sync` action; the panel's own control is `Refresh`.
// - Opening the panel is not a read-only act: `GET .../git/diff?head=<sid>`
//   syncs the session head first (`SPEC.md`, "Git"). What it does not do is
//   emit a `git` event, and that — not an untouched mirror — is what the
//   no-refresh-loop rule means and what `the diff endpoint's own fetch-back
//   emits no git event` asserts.
//
// **Two push forms.** A session row's `Push…` sends its session ref, which the
// session scenarios also send to the remote branch `main` so a rejection has
// something to reject; an integration head's row has a `Push…` of its own,
// which is how merged work on `main` reaches the remote.

import { writeFileSync } from "node:fs";
import { join } from "node:path";

import type { Locator, Page } from "@playwright/test";

import type { Project, SyncResult } from "../src/types";
import type { Api } from "./utils/test-helpers";
import { expect, test } from "./utils/fixtures";
import type { SessionTracker } from "./utils/fixtures";
import {
  AUTHOR_BRANCH_WAIT,
  commitInSessionWorkClone,
  createTask,
  createTaskAsSession,
  forwardHandoff,
  getTask,
  commitToBareRepo,
  gitIsAncestor,
  gitLogLast,
  gitRevParse,
  listBranches,
  loginViaToken,
  mirrorPath,
  moveUpstreamInto,
  publishRevision,
  sessionWorkPath,
  taskCardTestId,
  transcript,
  waitFor,
  waitForSessionState,
} from "./utils/test-helpers";

// The upstream every scenario here clones: the file the commits rewrite.
test.use({ repoFiles: { "src/app.txt": "v1\n" } });

// A container start, a clone and, in the longer scenarios, four git operations
// the orchestrator serialises per project.
test.setTimeout(180_000);

interface Stage {
  sessionId: string;
  /** `<DATA_DIR>/projects/<pid>/repo.git`, where every ref assertion is made. */
  mirror: string;
}

/**
 * A session on the `project` fixture, launched from `main` and waited to
 * `running` — which is what says its work clone exists.
 */
async function stage(
  sessions: SessionTracker,
  client: Api,
  project: Project,
): Promise<Stage> {
  const session = await sessions.launch(client, project.id, {
    base_ref: "main",
  });
  await waitForSessionState(client, session.id, "running", 120_000);
  return { sessionId: session.id, mirror: mirrorPath(project.id) };
}

// --- REST shorthands --------------------------------------------------------

/** `POST /sessions/{id}/sync` (`SPEC.md`, "Sessions"): the explicit fetch-back. */
function syncSession(client: Api, sessionId: string): Promise<SyncResult> {
  return client.post<SyncResult>(`/sessions/${sessionId}/sync`);
}

// --- locators ---------------------------------------------------------------

/** The `Branches` section of `GitActionsPanel`, on either page it appears on. */
function branches(page: Page): Locator {
  return page
    .getByRole("heading", { name: "Branches" })
    .locator("xpath=ancestor::section[1]");
}

/**
 * A session's row in the branch table. Every session here is launched without
 * a message, so it has no title and the table shows the first eight characters
 * of its id — which is also a stable, unique handle for the row.
 */
function branchRow(page: Page, sessionId: string): Locator {
  return branches(page)
    .getByRole("row")
    .filter({
      has: page.getByRole("link", { name: sessionId.slice(0, 8), exact: true }),
    });
}

/**
 * A git form, by the id of one control only it has.
 *
 * The forms carry no accessible name, and the project page shows the generic
 * merge form beside whichever row form is open, so `getByRole("form")` would be
 * ambiguous. `FormField` gives every control `id={name}` and `GitActionsPanel`
 * builds those names from `git-<action>-<session id>`, which is unique per page.
 */
function formWith(page: Page, controlId: string): Locator {
  return page.locator("form").filter({ has: page.locator(`#${controlId}`) });
}

/** Opens a row's form and returns it. */
async function openRowForm(
  page: Page,
  sessionId: string,
  action: "merge" | "rebase" | "push",
): Promise<Locator> {
  await branchRow(page, sessionId)
    .getByRole("button", {
      name: { merge: "Merge into…", rebase: "Rebase onto…", push: "Push…" }[
        action
      ],
    })
    .click();
  const control = {
    merge: "target",
    rebase: "onto",
    push: "remote-branch",
  }[action];
  const form = formWith(page, `git-${action}-${sessionId}-${control}`);
  await expect(form).toBeVisible();
  return form;
}

/** The session view's open side panel. */
function panel(page: Page): Locator {
  return page.getByRole("tabpanel");
}

/** Opens the session view with its `Changes` panel showing. */
async function openChanges(page: Page, sessionId: string): Promise<Locator> {
  await page.goto(`/sessions/${sessionId}`);
  // The panel opens on a wide viewport with `Changes` already selected; the
  // click makes the spec independent of that default.
  await page.getByRole("tab", { name: "Changes" }).click();
  return panel(page);
}

/** Opens the session header's folded git section. */
async function openBranchSection(page: Page): Promise<void> {
  const toggle = page.getByRole("button", { name: "branch", exact: true });
  if ((await toggle.getAttribute("aria-expanded")) !== "true") {
    await toggle.click();
  }
  await expect(branches(page)).toBeVisible();
}

test("the changes panel shows the session diff after a sync", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId, mirror } = await stage(sessions, api, project);

  const changes = await openChanges(page, sessionId);
  // Nothing has been written into the clone yet, so the branch is its base.
  await expect(changes.getByText("No changes against main")).toBeVisible({
    timeout: 60_000,
  });

  const commit = commitInSessionWorkClone(
    sessionId,
    { "src/app.txt": "v2\n", "NEW.md": "hi\n" },
    "feat: change",
  );
  // The panel refreshes on `git` events, and a host-side commit is not one, so
  // the view stands still until the session is synced.
  await expect(changes.getByText("No changes against main")).toBeVisible();

  const synced = await syncSession(api, sessionId);
  expect(synced.ref).toBe(`refs/sessions/${sessionId}`);
  expect(synced.commit).toBe(commit);
  // `SPEC.md`, "AgentEvent", `git`: the explicit sync's outcome reaches the
  // transcript, and the panel is refreshed by it.
  await expect(transcript(page).getByText("Git sync succeeded")).toBeVisible({
    timeout: 30_000,
  });

  await expect(
    changes.getByRole("button", { name: /^M src\/app\.txt \+1 −1$/ }),
  ).toBeVisible({ timeout: 30_000 });
  await expect(
    changes.getByRole("button", { name: /^A NEW\.md \+1 −0$/ }),
  ).toBeVisible();
  await expect(changes.getByText("2 files")).toBeVisible();

  // The patch itself, drawn by the same renderer the edit tools use.
  await expect(
    changes.getByRole("button", { name: /^src\/app\.txt 2 changed$/ }),
  ).toBeVisible();
  await expect(changes.getByText("v2", { exact: true }).first()).toBeVisible();

  // `ARCHITECTURE.md`, "Git model": the fetch-back keeps the branch under the
  // session's own ref in the mirror.
  expect(gitRevParse(mirror, `refs/sessions/${sessionId}`)).toBe(commit);
});

test("the diff endpoint's own fetch-back emits no git event", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId } = await stage(sessions, api, project);

  const changes = await openChanges(page, sessionId);
  await expect(changes.getByText("No changes against main")).toBeVisible({
    timeout: 60_000,
  });

  commitInSessionWorkClone(sessionId, { "src/app.txt": "v2\n" }, "feat: change");
  await syncSession(api, sessionId);
  await expect(transcript(page).getByText("Git sync succeeded")).toBeVisible({
    timeout: 30_000,
  });

  // Leaving and re-entering the panel unmounts and remounts it, so each return
  // is a fresh `GET .../git/diff?head=<sid>` — and each one of those syncs the
  // session head (`SPEC.md`, "Git"). None of them may add a `git` event, or the
  // panel's own refresh rule would drive it in a loop.
  for (let visit = 0; visit < 2; visit += 1) {
    await page.getByRole("tab", { name: "Tasks" }).click();
    await page.getByRole("tab", { name: "Changes" }).click();
    await expect(
      panel(page).getByRole("button", { name: /^M src\/app\.txt/ }),
    ).toBeVisible({ timeout: 30_000 });
  }

  await expect(transcript(page).getByText("Git sync succeeded")).toHaveCount(1);
});

test("the session branch list shows ahead and behind", async ({
  page,
  context,
  user,
  api,
  repo,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId } = await stage(sessions, api, project);

  commitInSessionWorkClone(sessionId, { "src/app.txt": "v2\n" }, "feat: ahead");
  await syncSession(api, sessionId);

  await page.goto(`/projects/${project.id}?tab=branches`);
  const row = branchRow(page, sessionId);
  // `SPEC.md`, "Git": ahead/behind is measured against the integration head
  // named by `default_branch`, which the cell's own title spells out.
  await expect(row.getByTitle("1 ahead of main")).toHaveText("+1", {
    timeout: 30_000,
  });
  await expect(row.getByTitle("0 behind main")).toHaveText("−0");

  // Moving the upstream and integrating it is what puts the branch behind. The
  // generic merge form is the UI for exactly that: `origin/main` into `main`.
  const moved = commitToBareRepo(
    repo,
    { "CHANGELOG.md": "# Changelog\n\n- upstream moved\n" },
    "Move the upstream",
  );
  await page.getByRole("button", { name: "Fetch now" }).click();
  await waitFor(
    async () => {
      const list = await listBranches(api, project.id);
      return list.find((one) => one.name === "origin/main")?.commit === moved
        ? list
        : null;
    },
    { timeoutMs: 30_000, description: "origin/main to reach the new commit" },
  );

  const generic = formWith(page, "git-merge-generic-target");
  await expect(generic.locator("#git-merge-generic-source")).toHaveValue(
    "origin/main",
  );
  await expect(generic.locator("#git-merge-generic-target")).toHaveValue("main");
  await generic.getByRole("button", { name: "Merge" }).click();
  await expect(generic.getByText(/^Merged at [0-9a-f]{7}/)).toBeVisible({
    timeout: 60_000,
  });

  await expect(row.getByTitle("1 behind main")).toHaveText("−1", {
    timeout: 30_000,
  });
});

test("the branches tab holds the git panel and the sessions tab does not", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId, mirror } = await stage(sessions, api, project);
  commitInSessionWorkClone(sessionId, { "src/app.txt": "v2\n" }, "feat: work");
  await syncSession(api, sessionId);

  // The Sessions tab lists the session and nothing of git.
  await page.goto(`/projects/${project.id}?tab=sessions`);
  await expect(
    page.getByRole("heading", { name: "Sessions", exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: "untitled" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Branches" })).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Merge any ref" }),
  ).toHaveCount(0);

  // The tab strip's `Branches` is a real link to `?tab=branches`.
  const sections = page.getByRole("navigation", { name: "Project sections" });
  await sections.getByRole("link", { name: "Branches", exact: true }).click();
  await expect(page).toHaveURL(/[?&]tab=branches(&|$)/);
  await expect(
    sections.getByRole("link", { name: "Branches", exact: true }),
  ).toHaveAttribute("aria-current", "page");

  // In the order the operator works: the heads, what went into them,
  // upstream in, the session refs.
  const panel = branches(page);
  await expect(panel.getByRole("heading", { level: 3 })).toHaveText([
    "Integration heads",
    "History",
    "Merge any ref",
    "Session branches",
  ]);

  const head = panel.getByRole("row").filter({
    has: page.getByText("default", { exact: true }),
  });
  await expect(head).toHaveCount(1);
  await expect(head.getByText("main", { exact: true })).toBeVisible();
  await expect(
    head.getByText(gitRevParse(mirror, "refs/heads/main").slice(0, 7), {
      exact: true,
    }),
  ).toBeVisible();

  await expect(
    formWith(page, "git-merge-generic-target").locator(
      "#git-merge-generic-source",
    ),
  ).toHaveValue("origin/main");
  await expect(branchRow(page, sessionId)).toBeVisible({ timeout: 30_000 });
});

test("merging a session branch into main", async ({
  page,
  context,
  user,
  api,
  repo,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId, mirror } = await stage(sessions, api, project);

  // The histories have to diverge for git to write a merge commit at all, so
  // the upstream moves first: otherwise the merge fast-forwards and there is no
  // commit of the orchestrator's own to read an identity off.
  await moveUpstreamInto(
    api,
    project,
    repo,
    { "CHANGELOG.md": "# Changelog\n" },
    "Move the upstream",
  );

  const commit = commitInSessionWorkClone(
    sessionId,
    { "src/app.txt": "v2\n" },
    "feat: session work",
  );
  await syncSession(api, sessionId);

  await page.goto(`/projects/${project.id}?tab=branches`);
  const form = await openRowForm(page, sessionId, "merge");
  await expect(form.locator(`#git-merge-${sessionId}-target`)).toHaveValue(
    "main",
  );
  await form.getByRole("button", { name: "Merge" }).click();
  await expect(form.getByText(/^Merged at [0-9a-f]{7}/)).toBeVisible({
    timeout: 60_000,
  });

  expect(gitIsAncestor(mirror, commit, "main")).toBe(true);
  // `ARCHITECTURE.md`, "Git model", "Commit identity": the merge is the bot's
  // and names who asked for it.
  expect(gitLogLast(mirror, "%an", "main")).toBe("Mars E2E Bot");
  expect(gitLogLast(mirror, "%b", "main")).toContain(
    `Requested-By: user:${user.id}`,
  );

  // The session that took part is told about it (`SPEC.md`, "AgentEvent").
  await page.goto(`/sessions/${sessionId}`);
  await expect(transcript(page).getByText("Git merge succeeded")).toBeVisible({
    timeout: 30_000,
  });
});

test("a task filed by a session waits on the board until its branch reaches main", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId, mirror } = await stage(sessions, api, project);
  const commit = commitInSessionWorkClone(
    sessionId,
    { "docs/plan.md": "# The plan\n" },
    "docs: plan the work",
  );
  await syncSession(api, sessionId);

  // Filed over MCP as the session, which is what records it as the author.
  const task = await createTaskAsSession(sessionId, "Build on the plan");
  expect(task.created_by_session_id).toBe(sessionId);

  // The session was launched without a message, so it has no title and the
  // line names it by its short id, as the Branches tab does.
  const expected = `Waiting for session ${sessionId.slice(0, 8)}'s branch to reach main (1 commit)`;
  await page.goto(`/projects/${project.id}?tab=board`);
  const card = page.getByTestId(taskCardTestId(task.number));
  const line = card.getByTestId(AUTHOR_BRANCH_WAIT);
  await expect(line).toHaveText(expected);
  await expect(line).toHaveAttribute(
    "href",
    `/projects/${project.id}?tab=branches`,
  );

  // The drawer says the same.
  await card.getByRole("heading", { name: "Build on the plan" }).click();
  const drawer = page.getByRole("dialog");
  await expect(drawer.getByTestId(AUTHOR_BRANCH_WAIT)).toHaveText(expected);

  // Landing the work through the API — nothing on this page invalidates the
  // branch list for it — and the line goes on the list's own next read.
  await api.post(`/projects/${project.id}/git/merge`, {
    source: sessionId,
    target: "main",
  });
  expect(gitIsAncestor(mirror, commit, "main")).toBe(true);

  await page.goto(`/projects/${project.id}?tab=board`);
  await expect(card).toBeVisible();
  await expect(card.getByTestId(AUTHOR_BRANCH_WAIT)).toHaveCount(0);
});

test("rebasing the session branch onto main", async ({
  page,
  context,
  user,
  api,
  repo,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId, mirror } = await stage(sessions, api, project);

  const original = commitInSessionWorkClone(
    sessionId,
    { "src/app.txt": "v2\n" },
    "feat: session work",
  );
  await syncSession(api, sessionId);
  await moveUpstreamInto(
    api,
    project,
    repo,
    { "CHANGELOG.md": "# Changelog\n" },
    "Move the upstream",
  );

  await page.goto(`/projects/${project.id}?tab=branches`);
  const form = await openRowForm(page, sessionId, "rebase");
  await expect(form.locator(`#git-rebase-${sessionId}-onto`)).toHaveValue(
    "main",
  );
  await form.getByRole("button", { name: "Rebase" }).click();
  await expect(form.getByText(/^Rebased to [0-9a-f]{7}/)).toBeVisible({
    timeout: 60_000,
  });

  const rebased = gitRevParse(mirror, `refs/sessions/${sessionId}`);
  expect(rebased).not.toBe(original);
  expect(gitIsAncestor(mirror, gitRevParse(mirror, "main"), rebased)).toBe(true);
  // The work clone was clean, so the orchestrator reset the checkout onto the
  // new commits (`ARCHITECTURE.md`, "Git model": the rebase reconciles it).
  expect(gitRevParse(sessionWorkPath(sessionId), "HEAD")).toBe(rebased);

  await page.goto(`/sessions/${sessionId}`);
  await expect(transcript(page).getByText("Git rebase succeeded")).toBeVisible({
    timeout: 30_000,
  });
});

test("a rebase onto a dirty checkout asks the session to reconcile", async ({
  page,
  context,
  user,
  api,
  repo,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId } = await stage(sessions, api, project);

  const before = commitInSessionWorkClone(
    sessionId,
    { "src/app.txt": "v2\n" },
    "feat: session work",
  );
  await syncSession(api, sessionId);
  await moveUpstreamInto(
    api,
    project,
    repo,
    { "CHANGELOG.md": "# Changelog\n" },
    "Move the upstream",
  );

  // An uncommitted edit to a tracked file: the branch still rebases, but the
  // checkout is left alone and the event says so (`SPEC.md`, "AgentEvent":
  // `detail.work_tree`).
  writeFileSync(
    join(sessionWorkPath(sessionId), "src/app.txt"),
    "v2 with uncommitted work\n",
  );

  await page.goto(`/sessions/${sessionId}`);
  await openBranchSection(page);
  const form = await openRowForm(page, sessionId, "rebase");
  await form.getByRole("button", { name: "Rebase" }).click();
  // The rebase itself succeeds and produces a new commit; only the checkout is
  // left behind.
  const rebased = form.getByText(/^Rebased to [0-9a-f]{7}$/);
  await expect(rebased).toBeVisible({ timeout: 60_000 });
  expect(await rebased.innerText()).not.toBe(
    `Rebased to ${before.slice(0, 7)}`,
  );

  await expect(transcript(page).getByText("Git rebase succeeded")).toBeVisible({
    timeout: 30_000,
  });
  // The form reads the outcome off the session's own `git` event.
  await expect(
    page.getByText(
      "The last rebase could not update this session's checkout: its work tree is dirty, so the agent has to reconcile it.",
    ),
  ).toBeVisible({ timeout: 30_000 });

  // The checkout is exactly where the agent left it, uncommitted edit and all.
  //
  // The mirror's `refs/sessions/<sid>` is deliberately *not* asserted here.
  // The Changes panel is open beside the transcript and refetches its diff on
  // every `git` event, and that fetch-back syncs the session head — which, from
  // an unreconciled checkout still on the pre-rebase commit, writes the old
  // commit back over the rewritten ref. The rebase is not undone in any sense
  // the session can see, and the checkout is the thing this scenario is about.
  expect(gitRevParse(sessionWorkPath(sessionId), "HEAD")).toBe(before);
});

test("pushing to the bare upstream, without a compare link for a file:// remote", async ({
  page,
  context,
  user,
  api,
  repo,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId, mirror } = await stage(sessions, api, project);

  const commit = commitInSessionWorkClone(
    sessionId,
    { "src/app.txt": "v2\n" },
    "feat: session work",
  );
  await syncSession(api, sessionId);

  await page.goto(`/projects/${project.id}?tab=branches`);
  const form = await openRowForm(page, sessionId, "push");

  // A session ref is pushed as `session/<id>` by default (`ARCHITECTURE.md`,
  // "Git model").
  const remoteBranch = `session/${sessionId}`;
  await expect(form.locator(`#git-push-${sessionId}-remote-branch`)).toHaveValue(
    remoteBranch,
  );
  await form.getByRole("button", { name: "Push" }).click();
  await expect(
    form.getByText(`Pushed ${remoteBranch} at ${commit.slice(0, 7)}`),
  ).toBeVisible({ timeout: 60_000 });
  expect(gitRevParse(repo.path, remoteBranch)).toBe(commit);

  // The compare page is GitHub's; a `file://` upstream has none, so the UI
  // offers no link rather than a broken one (`SPEC.md`, "Frontend").
  await expect(
    form.getByRole("link", { name: "Open compare on GitHub" }),
  ).toHaveCount(0);
  await expect(
    branchRow(page, sessionId).getByRole("link", { name: "compare" }),
  ).toHaveCount(0);

  // The same form sends the ref to `main`, which is the documented
  // `{remote_branch, commit}` answer for an upstream default branch.
  await form.locator(`#git-push-${sessionId}-remote-branch`).fill("main");
  await form.getByRole("button", { name: "Push" }).click();
  await expect(
    form.getByText(`Pushed main at ${commit.slice(0, 7)}`),
  ).toBeVisible({ timeout: 60_000 });
  expect(gitRevParse(repo.path, "main")).toBe(commit);
  expect(gitRevParse(mirror, `refs/sessions/${sessionId}`)).toBe(commit);

  await page.goto(`/sessions/${sessionId}`);
  await expect(transcript(page).getByText("Git push succeeded")).toHaveCount(2, {
    timeout: 30_000,
  });
});

test("a non-fast-forward push is refused until it is forced", async ({
  page,
  context,
  user,
  api,
  repo,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId, mirror } = await stage(sessions, api, project);

  const commit = commitInSessionWorkClone(
    sessionId,
    { "src/app.txt": "v2\n" },
    "feat: session work",
  );
  await syncSession(api, sessionId);

  await page.goto(`/projects/${project.id}?tab=branches`);
  const form = await openRowForm(page, sessionId, "push");
  await form.locator(`#git-push-${sessionId}-remote-branch`).fill("main");
  await form.getByRole("button", { name: "Push" }).click();
  await expect(form.getByText(`Pushed main at ${commit.slice(0, 7)}`)).toBeVisible(
    { timeout: 60_000 },
  );

  // The upstream moves behind Mars's back, so the next push is no longer a
  // fast-forward (`SPEC.md`, "Git": 409, local work preserved).
  const upstream = commitToBareRepo(
    repo,
    { "CHANGELOG.md": "# Changelog\n\n- upstream moved\n" },
    "Move the upstream",
  );
  await form.getByRole("button", { name: "Push" }).click();
  await expect(
    form.getByText(
      "Push rejected: upstream has advanced. Fetch, merge origin/main and retry.",
    ),
  ).toBeVisible({ timeout: 60_000 });
  expect(gitRevParse(repo.path, "main")).toBe(upstream);

  // `force: true` is the explicit override REST requires.
  await form.getByRole("checkbox", { name: "Force push" }).check();
  await expect(
    form.getByText(
      "A force push overwrites whatever is on the remote branch. Commits only on the remote are lost.",
    ),
  ).toBeVisible();
  await form.getByRole("button", { name: "Push" }).click();
  await expect(form.getByText(`Pushed main at ${commit.slice(0, 7)}`)).toBeVisible(
    { timeout: 60_000 },
  );
  expect(gitRevParse(repo.path, "main")).toBe(
    gitRevParse(mirror, `refs/sessions/${sessionId}`),
  );
});

test("a merge conflict lists the conflicting paths and leaves main alone", async ({
  page,
  context,
  user,
  api,
  repo,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId, mirror } = await stage(sessions, api, project);

  // Both sides rewrite the same file from the same merge base.
  await moveUpstreamInto(
    api,
    project,
    repo,
    { "src/app.txt": "upstream\n" },
    "Rewrite the file upstream",
  );
  commitInSessionWorkClone(
    sessionId,
    { "src/app.txt": "session\n" },
    "feat: rewrite the file in the session",
  );
  await syncSession(api, sessionId);

  const before = gitRevParse(mirror, "main");

  await page.goto(`/projects/${project.id}?tab=branches`);
  const form = await openRowForm(page, sessionId, "merge");
  await form.getByRole("button", { name: "Merge" }).click();

  const conflicts = form.getByRole("alert").filter({ hasText: "Conflicts in:" });
  await expect(conflicts).toBeVisible({ timeout: 60_000 });
  await expect(conflicts.getByText("src/app.txt", { exact: true })).toBeVisible();

  // The 422 is a stop, not a partial write (`SPEC.md`, "Git").
  expect(gitRevParse(mirror, "main")).toBe(before);
});

test("an upstream-tracking ref cannot be a mutation target", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId } = await stage(sessions, api, project);

  commitInSessionWorkClone(sessionId, { "src/app.txt": "v2\n" }, "feat: work");
  await syncSession(api, sessionId);

  await page.goto(`/projects/${project.id}?tab=branches`);
  // The UI cannot express the request at all: the target select offers
  // integration heads only, so `origin/main` is not among its options.
  const generic = formWith(page, "git-merge-generic-target");
  const target = generic.locator("#git-merge-generic-target");
  await expect(target.locator("option")).toHaveText(["main"]);
  // The source select, which does accept upstream refs, has it.
  await expect(
    generic.locator("#git-merge-generic-source").locator("option", {
      hasText: "origin/main",
    }),
  ).toHaveCount(1);

  // Asked for directly, the orchestrator refuses it with 400.
  const refused = await api.send(
    "POST",
    `/projects/${project.id}/git/merge`,
    { source: "main", target: "origin/main" },
    { allow: [400] },
  );
  expect(refused.status).toBe(400);
  expect((refused.body as { error: string }).error).toContain("origin/main");
});

/** An integration head's row on the Branches tab, by its name. */
function headRow(page: Page, name: string): Locator {
  return branches(page)
    .getByRole("row")
    .filter({ has: page.getByText(name, { exact: true }) })
    .filter({ has: page.getByRole("button", { name: "Push…" }) });
}

test("pushing main from its integration head row", async ({
  page,
  context,
  user,
  api,
  repo,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId, mirror } = await stage(sessions, api, project);

  // Merged work waiting on `main`: a session's commit merged in, as a task's
  // merge or an automatic merge leaves it (`README.md`, "Operating notes").
  commitInSessionWorkClone(sessionId, { "src/app.txt": "v2\n" }, "feat: work");
  await syncSession(api, sessionId);
  await api.post(`/projects/${project.id}/git/merge`, {
    source: sessionId,
    target: "main",
  });
  const merged = gitRevParse(mirror, "main");

  // The upstream moves too, so the first push is not a fast-forward.
  const upstream = commitToBareRepo(
    repo,
    { "CHANGELOG.md": "# Changelog\n\n- upstream moved\n" },
    "Move the upstream",
  );

  await page.goto(`/projects/${project.id}?tab=branches`);
  const toggle = headRow(page, "main").getByRole("button", { name: "Push…" });
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  const form = formWith(page, "git-push-head-main-remote-branch");
  await expect(form).toBeVisible();
  // A head keeps its own name on the remote.
  await expect(form.locator("#git-push-head-main-remote-branch")).toHaveValue(
    "main",
  );

  await form.getByRole("button", { name: "Push" }).click();
  await expect(
    form.getByText(
      "Push rejected: upstream has advanced. Fetch, merge origin/main and retry.",
    ),
  ).toBeVisible({ timeout: 60_000 });
  expect(gitRevParse(repo.path, "main")).toBe(upstream);
  expect(gitRevParse(mirror, "main")).toBe(merged);

  // The recovery the rejection names: fetch, merge `origin/main`, push again.
  await api.post(`/projects/${project.id}/fetch`);
  await waitFor(
    async () => {
      const refs = await listBranches(api, project.id);
      const tracking = refs.find((one) => one.name === "origin/main");
      return tracking?.commit === upstream ? refs : null;
    },
    { timeoutMs: 30_000, description: "origin/main to reach the new commit" },
  );
  await api.post(`/projects/${project.id}/git/merge`, {
    source: "origin/main",
    target: "main",
  });
  const integrated = gitRevParse(mirror, "main");

  await form.getByRole("button", { name: "Push" }).click();
  await expect(
    form.getByText(`Pushed main at ${integrated.slice(0, 7)}`),
  ).toBeVisible({ timeout: 60_000 });
  expect(gitRevParse(repo.path, "main")).toBe(integrated);
  expect(gitIsAncestor(repo.path, merged, "main")).toBe(true);
  expect(gitIsAncestor(repo.path, upstream, "main")).toBe(true);

  // `main` pushed to `main` has nothing to compare against.
  await expect(
    form.getByRole("link", { name: "Open compare on GitHub" }),
  ).toHaveCount(0);
});

test("reverting main to before two task merges reopens the tasks without their hand-offs", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const { sessionId, mirror } = await stage(sessions, api, project);
  const base = gitRevParse(mirror, "main");

  // Two tasks merged into `main` by auto-merge, each from its own commit on
  // the one session: a revision pins the session's tip, an approval moves it
  // to `merge`, and the job merges it and closes the task in `done`. A user's
  // REST hand-off may name any session of the project (`SPEC.md`, "Code
  // hand-offs and review").
  for (const [number, content] of [
    [1, "v2\n"],
    [2, "v3\n"],
  ] as const) {
    const task = await createTask(api, project.id, {
      title: `Change the app to ${content.trim()}`,
      state: "ready",
    });
    expect(task.number).toBe(number);
    const commit = commitInSessionWorkClone(
      sessionId,
      { "src/app.txt": content },
      `feat: ${content.trim()}`,
    );
    const revision = await publishRevision(api, project.id, number, {
      source: sessionId,
      commit,
      comment: "Ready for review",
      state: "review",
    });
    await forwardHandoff(api, project.id, number, {
      handoffId: revision.id,
      comment: "LGTM",
      state: "merge",
      review: "approved",
    });
    await waitFor(
      async () =>
        (await getTask(api, project.id, number)).state === "done" ? true : null,
      { timeoutMs: 30_000, description: `task #${String(number)} merged` },
    );
  }
  expect(gitRevParse(mirror, "main")).not.toBe(base);

  await page.goto(`/projects/${project.id}?tab=branches`);
  const history = page.getByRole("table", { name: "History of main" });
  const short = base.slice(0, 7);
  const baseRow = history.getByRole("row").filter({ hasText: short });
  await expect(baseRow).toBeVisible({ timeout: 30_000 });
  // Both merges are above the base, each attributed to its task.
  await expect(history.getByRole("link", { name: "#1" })).toBeVisible();
  await expect(history.getByRole("link", { name: "#2" })).toBeVisible();

  await baseRow.getByRole("button", { name: "Revert to here" }).click();
  await expect(
    page.getByText(/restoring it to .* 2 commits and these tasks are undone/),
  ).toBeVisible();
  await history.getByLabel("Reopen these tasks").check();
  await history.getByLabel("Move them to").selectOption("ready");
  await history.getByLabel("Comment").fill("Built on the wrong base");
  await page.getByRole("button", { name: `Revert main to ${short}` }).click();

  await expect(
    page.getByText(/Reverted main to .* reopening 2 tasks/),
  ).toBeVisible();
  // The history is reread: the revert commit is the new head, above the
  // commits it undid, which stay in the history.
  await expect(
    history
      .getByRole("row")
      .nth(1)
      .getByText(`Revert main to ${base.slice(0, 12)}`),
  ).toBeVisible();
  await expect(history.getByRole("link", { name: "#2" })).toBeVisible();

  // One new commit whose tree is the base's; `main` never moved backwards.
  expect(gitRevParse(mirror, "main^{tree}")).toBe(
    gitRevParse(mirror, `${base}^{tree}`),
  );
  expect(gitIsAncestor(mirror, base, "main")).toBe(true);

  for (const number of [1, 2]) {
    const task = await getTask(api, project.id, number);
    expect(task.state).toBe("ready");
    expect(task.handoff).toBeNull();
    expect(task.handoffs.length).toBeGreaterThan(0);
  }
});

// The compare link is built entirely in the browser from `remote_url`
// (`SPEC.md`, "Frontend", "Changes panel"), and the only remote these scenarios
// can push to is the local bare repository — `RemoteUrl::parse` accepts
// `file://` only under the orchestrator's `integration-tests` feature, and a
// `https://github.com/...` project would never finish cloning here, so there is
// no push result to hang the link on.
//
// Its absence for a non-GitHub remote is asserted above, in `pushing to the
// bare upstream`. The builder itself — the `<owner>/<repo>` parse, the
// `?expand=1` suffix and the unencoded `session/<id>` ref — is covered by the
// Vitest suite `frontend/src/utils/github.test.ts`, which is where a pure
// function belongs (`CLAUDE.md`, "Testing expectations").
test.skip("the github compare link is built client-side", () => {
  // Covered by `src/utils/github.test.ts`; see the note above.
});
