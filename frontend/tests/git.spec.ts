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
// **Three places the task text and the application disagree**, each asserted as
// the application is built:
//
// - There is no branches tab. `GitActionsPanel` sits at the bottom of the
//   project's *Sessions* tab (`?tab=sessions`) and, filtered to one row, behind
//   the session header's `branch` toggle.
// - The Changes panel has no Sync of its own. The explicit sync is the session
//   header's `Sync` action; the panel's own control is `Refresh`.
// - Opening the panel is not a read-only act: `GET .../git/diff?head=<sid>`
//   syncs the session head first (`SPEC.md`, "Git"). What it does not do is
//   emit a `git` event, and that — not an untouched mirror — is what the
//   no-refresh-loop rule means and what `the diff endpoint's own fetch-back
//   emits no git event` asserts.
//
// **The UI can only push a session ref.** `GitActionsPanel` renders `PushForm`
// for branch rows alone, so `push main upstream` is done the way the form
// allows: the session ref pushed to the remote branch `main`, which is the same
// `POST .../git/push` body with the same `{remote_branch, commit}` answer.

import { writeFileSync } from "node:fs";
import { join } from "node:path";

import type { Locator, Page } from "@playwright/test";

import type { Project, SyncResult } from "../src/types";
import type { Api, BareRepo } from "./utils/test-helpers";
import { expect, test } from "./utils/fixtures";
import type { SessionTracker } from "./utils/fixtures";
import {
  commitInSessionWorkClone,
  commitToBareRepo,
  gitIsAncestor,
  gitLogLast,
  gitRevParse,
  listBranches,
  loginViaToken,
  mirrorPath,
  sessionWorkPath,
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

/**
 * Moves the upstream and waits until the project's `origin/<branch>` has caught
 * up, then integrates it into the matching head — the documented way fetched
 * upstream changes become Mars's own (`SPEC.md`, "Git": merging `origin/main`
 * into `main`).
 *
 * The scenarios that assert *how* this looks in the UI do it through the forms;
 * this is the arrangement the merge, rebase and conflict scenarios need before
 * their own assertion begins, so it goes the short way.
 */
async function moveUpstreamInto(
  client: Api,
  project: Project,
  repo: BareRepo,
  files: Record<string, string>,
  message: string,
): Promise<string> {
  const moved = commitToBareRepo(repo, files, message);
  await client.post(`/projects/${project.id}/fetch`);
  await waitFor(
    async () => {
      const branches = await listBranches(client, project.id);
      const upstream = branches.find((one) => one.name === "origin/main");
      return upstream?.commit === moved ? branches : null;
    },
    { timeoutMs: 30_000, description: "origin/main to reach the new commit" },
  );
  await client.post(`/projects/${project.id}/git/merge`, {
    source: "origin/main",
    target: "main",
  });
  return moved;
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

  await page.goto(`/projects/${project.id}?tab=sessions`);
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

  await page.goto(`/projects/${project.id}?tab=sessions`);
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

  await page.goto(`/projects/${project.id}?tab=sessions`);
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

  await page.goto(`/projects/${project.id}?tab=sessions`);
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

  await page.goto(`/projects/${project.id}?tab=sessions`);
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

  await page.goto(`/projects/${project.id}?tab=sessions`);
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

  await page.goto(`/projects/${project.id}?tab=sessions`);
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
