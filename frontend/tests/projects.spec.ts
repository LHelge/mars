// The "Projects" and "Agent profiles" feature paragraphs of `SPEC.md`,
// "User-facing features", driven through the browser against a real
// orchestrator: a project from a local bare repository to `ready`, a clone that
// fails and is retried, the fetch that moves the upstream ref and leaves the
// integration head, the profile editor, the shared directories and their two
// refusals while a session is live, the settings form, and the delete.
//
// Every test makes its own user and its own upstream repository, so the file
// can be run repeatedly against one stack and in any order.
//
// Two places where the application is deliberately narrower than the feature
// paragraph, and the scenarios say so where they rely on it:
//
// - **the create form is `https://` only.** `RemoteUrl::parse` accepts a
//   `file://` remote only under the orchestrator's `integration-tests` feature
//   (`orchestrator/src/models/project.rs`), and `ProjectCreateForm` mirrors the
//   released contract — `SPEC.md`, "Projects" — by refusing anything else
//   before the request. `the new-project form refuses a remote that is not
//   https` asserts exactly that, and every scenario that needs a cloned project
//   creates it over REST, which is the same `POST /projects` the form calls.
// - **the editor cannot send an invalid `serves_states` or `mcp_tools`.** Both
//   are checkbox groups over the project's queue states and the four gated
//   tools, with no free-text path, so the 400s of `SPEC.md`, "Agent profiles",
//   are asserted against the API and the editor is asserted to offer nothing
//   else.

import { existsSync } from "node:fs";
import { join } from "node:path";

import type { Page } from "@playwright/test";

import type { Profile, Project, Session, SharedDir } from "../src/types";
import { PROFILE_GATED_TOOLS } from "../src/types";
import { expect, test } from "./utils/fixtures";
import {
  commitToBareRepo,
  createBareRepoAt,
  createProject,
  dataDir,
  defaultProfile,
  gitRevParse,
  isMobile,
  listBranches,
  loginViaToken,
  PROFILE_AUTOMATION,
  randomSuffix,
  reposDir,
  uniqueName,
  waitFor,
  waitForSessionState,
  expectNoHorizontalOverflow,
} from "./utils/test-helpers";

// The upstream the `repo` fixture builds for this file.
test.use({ repoFiles: { "src/lib.rs": "pub fn ok() {}\n" } });

/** A `file://` clone of a fixture repository is fast; 60 s is the cliff. */
const CLONE_TIMEOUT = 60_000;

/** The project header, located by the one action only it has. */
function projectHeader(page: Page) {
  return page
    .locator("section")
    .filter({ has: page.getByRole("button", { name: "Fetch now" }) });
}

/** The row of `/projects` that links to `name`. */
function projectRow(page: Page, name: string) {
  return page
    .getByRole("row")
    .filter({ has: page.getByRole("link", { name, exact: true }) });
}

/** One of the project page's tabs, by its label in the tab strip. */
function projectTab(page: Page, label: string) {
  return page
    .getByRole("navigation", { name: "Project sections" })
    .getByRole("link", { name: label, exact: true });
}

/**
 * Say yes to the inline `ConfirmPanel` a destructive action opens. Its button
 * names what it is about to do — `Remove target`, not `Remove` — so the panel
 * is addressed by that name and never confused with the row that opened it.
 */
function confirm(page: Page, label: string) {
  return page.getByRole("button", { name: label, exact: true }).click();
}

test("the new-project form refuses a remote that is not https", async ({
  page,
  context,
  user,
  repo,
}) => {
  await loginViaToken(context, user);

  await page.goto("/projects");
  // The header and the empty list both offer the button; either opens the form.
  await page.getByRole("button", { name: "New project" }).first().click();

  const form = page.getByRole("form", { name: "New project" });
  await form.getByLabel("Name").fill(uniqueName("e2e-refused"));
  await form.getByLabel("Remote URL").fill(repo.url);
  await form.getByRole("button", { name: "Start clone" }).click();

  // The client-side half of the contract, before any request goes out.
  await expect(
    form.getByText("The remote URL must start with https://."),
  ).toBeVisible();
  await expect(page).toHaveURL(/\/projects$/);
});

test("a project created from a bare repository reaches ready without a reload", async ({
  page,
  context,
  user,
  api,
  repo,
}) => {
  await loginViaToken(context, user);

  const name = uniqueName("e2e-created");

  // `POST /projects` answers 201 with `status: cloning` (`SPEC.md`,
  // "Projects"); the row on the list then settles on its own.
  const created = await api.send("POST", "/projects", {
    name,
    remote_url: repo.url,
  });
  expect(created.status).toBe(201);
  const project = created.body as Project;
  expect(project.status).toBe("cloning");

  await page.goto("/projects");
  const row = projectRow(page, name);
  await expect(row).toBeVisible();
  // No reload: the list polls while anything is cloning.
  await expect(row.getByText("ready", { exact: true })).toBeVisible({
    timeout: CLONE_TIMEOUT,
  });

  await page.getByRole("link", { name, exact: true }).click();
  await expect(page.getByRole("heading", { name })).toBeVisible();

  const header = projectHeader(page);
  await expect(header.getByText("main", { exact: true })).toBeVisible();
  await expect(header.getByText(repo.url, { exact: true })).toBeVisible();
  // `last_fetched_at` is set when the clone finishes, so it is no longer the
  // em dash the list shows for a project that has never been fetched.
  await expect(header.getByText("just now", { exact: true })).toBeVisible();

  await expect(page.getByText("No sessions yet")).toBeVisible();

  await projectTab(page, "Profiles").click();
  // The four profiles a project is seeded with (`SPEC.md`, "Role profile
  // templates"; ADR 0051): `claude` carrying the `default` badge and serving
  // nothing, the planner a person launches, and the implementer and the
  // reviewer the dispatcher may launch by itself. No credential this project
  // can use unattended exists, so the chip is all the `auto-launch` does here.
  const seeded = [
    { name: "claude", automation: "manual" },
    { name: "planner", automation: "manual" },
    { name: "implementer", automation: "auto-launch" },
    { name: "reviewer", automation: "auto-launch" },
  ];
  for (const { name, automation } of seeded) {
    const row = page
      .getByRole("row")
      .filter({ has: page.getByText(name, { exact: true }) });
    await expect(row).toBeVisible();
    await expect(row.getByTestId(PROFILE_AUTOMATION)).toHaveText(automation);
  }
  const defaultRow = page
    .getByRole("row")
    .filter({ has: page.getByText("claude", { exact: true }) });
  await expect(defaultRow.getByText("default", { exact: true })).toHaveCount(1);
  await expect(defaultRow.getByText("nothing", { exact: true })).toBeVisible();
  // Those four and no other: every profile row, and only a profile row, has
  // an `Edit` button.
  await expect(
    page
      .getByRole("row")
      .filter({ has: page.getByRole("button", { name: "Edit", exact: true }) }),
  ).toHaveCount(seeded.length);

  await projectTab(page, "Shared directories").click();
  await expect(page.getByText("No shared directories yet")).toBeVisible();
});

test("a clone that fails shows its message and the retry succeeds", async ({
  page,
  context,
  user,
  api,
}) => {
  await loginViaToken(context, user);

  // Named before it exists: the retry is what makes it a repository. It lives
  // under the stack's repository directory so it goes with the stack.
  const path = join(reposDir(), `retry-${randomSuffix()}.git`);
  expect(existsSync(path)).toBe(false);

  const name = uniqueName("e2e-broken");
  const project = await api.post<Project>("/projects", {
    name,
    remote_url: `file://${path}`,
  });

  await page.goto(`/projects/${project.id}`);
  const header = projectHeader(page);
  await expect(header.getByText("error", { exact: true })).toBeVisible({
    timeout: CLONE_TIMEOUT,
  });

  // `status_message` says why, in the orchestrator's own words.
  const failure = page.getByRole("alert").first();
  await expect(failure).toBeVisible();
  await expect(failure).not.toBeEmpty();

  const repo = createBareRepoAt(path);
  await page.getByRole("button", { name: "Retry clone" }).click();

  await expect(header.getByText("ready", { exact: true })).toBeVisible({
    timeout: CLONE_TIMEOUT,
  });
  await expect(header.getByText("main", { exact: true })).toBeVisible();

  const branches = await listBranches(api, project.id);
  expect(branches.find((branch) => branch.name === "main")?.commit).toBe(
    repo.initialCommit,
  );
});

test("fetch now moves the upstream ref and leaves the integration head", async ({
  page,
  context,
  user,
  api,
  repo,
  project,
}) => {
  await loginViaToken(context, user);

  const before = await listBranches(api, project.id);
  expect(before.find((b) => b.name === "main")?.kind).toBe("head");
  expect(before.find((b) => b.name === "origin/main")?.kind).toBe("upstream");
  expect(before.find((b) => b.name === "origin/main")?.commit).toBe(
    repo.initialCommit,
  );

  const moved = commitToBareRepo(
    repo,
    { "CHANGELOG.md": "# Changelog\n\n- upstream moved\n" },
    "Move the upstream",
  );
  expect(gitRevParse(repo.path, "main")).toBe(moved);

  await page.goto(`/projects/${project.id}`);
  await page.getByRole("button", { name: "Fetch now" }).click();

  // The refreshed `last_fetched_at` is what says the fetch landed in the UI;
  // the refs themselves are read back out of the orchestrator.
  const after = await waitFor(
    async () => {
      const branches = await listBranches(api, project.id);
      const upstream = branches.find((b) => b.name === "origin/main");
      return upstream?.commit === moved ? branches : null;
    },
    { timeoutMs: 30_000, description: "origin/main to reach the new commit" },
  );

  // Fetching refreshes upstream-tracking refs without moving integration heads
  // (`SPEC.md`, "Projects").
  expect(after.find((b) => b.name === "main")?.commit).toBe(repo.initialCommit);
  await expect(
    projectHeader(page).getByText("just now", { exact: true }),
  ).toBeVisible();
});

test("the default profile is edited and an ephemeral one is created beside it", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await loginViaToken(context, user);

  await page.goto(`/projects/${project.id}?tab=profiles`);
  // The seeded default profile, `claude`, which serves no state (`SPEC.md`,
  // "Role profile templates").
  await page
    .getByRole("row")
    .filter({ has: page.getByText("claude", { exact: true }) })
    .getByRole("button", { name: "Edit" })
    .click();

  // The editor's own fields are addressed by id: it also carries a free-text
  // "Secret name to declare" box, which a label lookup for "Name" would match.
  const editor = page.getByRole("form", { name: "Edit claude" });
  await expect(editor.locator("#profile-name")).toHaveValue("claude");
  // Within the fieldset: `merge` is also the name of a git tool's checkbox.
  const servedStates = editor.getByRole("group", { name: "Served states" });
  for (const state of ["backlog", "ready", "review", "merge"]) {
    await expect(
      servedStates.getByRole("checkbox", { name: state, exact: true }),
    ).not.toBeChecked();
  }
  await editor.locator("#profile-name").fill("architect");
  await editor
    .locator("#profile-system-prompt")
    .fill("Plan the work; never write code.");
  await editor.getByRole("checkbox", { name: "backlog", exact: true }).check();
  await editor
    .getByRole("checkbox", { name: /Stream partial messages/ })
    .uncheck();
  await editor.locator("#profile-idle-timeout").fill("300");
  await editor.getByRole("button", { name: "Save profile" }).click();

  const architectRow = page
    .getByRole("row")
    .filter({ has: page.getByText("architect", { exact: true }) });
  await expect(architectRow).toBeVisible();

  // A reload is the real check that the PUT replaced the stored profile
  // rather than only the screen.
  await page.reload();
  await expect(
    architectRow.getByText("backlog", { exact: true }),
  ).toBeVisible();
  await expect(architectRow.getByText("300s", { exact: true })).toBeVisible();
  const stored = await defaultProfile(api, project.id);
  expect(stored.name).toBe("architect");
  expect(stored.serves_states).toEqual(["backlog"]);
  expect(stored.partial_messages).toBe(false);
  expect(stored.idle_timeout_secs).toBe(300);
  expect(stored.system_prompt).toBe("Plan the work; never write code.");

  await page.getByRole("button", { name: "New profile" }).click();
  const create = page.getByRole("form", { name: "New profile" });
  await create.locator("#profile-name").fill("oneshot");
  await create.locator("#profile-kind").selectOption("ephemeral");
  await create.getByRole("button", { name: "Create profile" }).click();

  const oneshotRow = page
    .getByRole("row")
    .filter({ has: page.getByText("oneshot", { exact: true }) });
  await expect(oneshotRow).toBeVisible();
  await expect(
    oneshotRow.getByText("ephemeral", { exact: true }),
  ).toBeVisible();
  await expect(architectRow).toBeVisible();

  // A project always keeps one default profile. The API answers 409
  // (`SPEC.md`, "Agent profiles") and the tab does not offer the button at
  // all, which is the same refusal one step earlier.
  await expect(
    architectRow.getByRole("button", { name: "Delete" }),
  ).toBeDisabled();
  const refused = await api.send(
    "DELETE",
    `/projects/${project.id}/profiles/${stored.id}`,
    undefined,
    { allow: [409] },
  );
  expect(refused.status).toBe(409);

  await oneshotRow.getByRole("button", { name: "Delete" }).click();
  await confirm(page, "Delete oneshot");
  await expect(oneshotRow).toHaveCount(0);
  await expect(architectRow).toBeVisible();
});

test("a profile started from the reviewer template is saved as reviewer-2", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await loginViaToken(context, user);

  // Nothing is deleted first: the project still has its seeded `reviewer`, so
  // the template's name is the one already taken (`SPEC.md`, "Frontend",
  // Role templates).
  await page.goto(`/projects/${project.id}?tab=profiles&profile=new`);
  const editor = page.getByRole("form", { name: "New profile" });

  await editor.locator("#profile-template").selectOption("reviewer");

  await expect(editor.locator("#profile-name")).toHaveValue("reviewer-2");
  await expect(editor.locator("#profile-system-prompt")).toHaveValue(
    /^You are a reviewer of this project\./,
  );
  await expect(
    editor.getByRole("checkbox", { name: "review", exact: true }),
  ).toBeChecked();
  await expect(
    editor.getByRole("checkbox", {
      name: "list_session_branches",
      exact: true,
    }),
  ).toBeChecked();
  // The seeded reviewer is ephemeral and auto-launched (ADR 0051), and the
  // template brings both with it.
  await expect(editor.locator("#profile-kind")).toHaveValue("ephemeral");
  await expect(
    editor.getByRole("checkbox", {
      name: /Let the dispatcher launch this profile/,
    }),
  ).toBeChecked();

  // `Blank` puts the defaults back: the pre-fill is a starting point, and
  // choosing it again is not an instantiation of anything.
  await editor.locator("#profile-template").selectOption("");
  await expect(editor.locator("#profile-name")).toHaveValue("");
  await expect(editor.locator("#profile-system-prompt")).toHaveValue("");
  await expect(
    editor.getByRole("checkbox", { name: "review", exact: true }),
  ).not.toBeChecked();

  await editor.locator("#profile-template").selectOption("reviewer");
  await editor.getByRole("button", { name: "Create profile" }).click();

  // This project has no agent credential an unattended launch could use — the
  // `api` fixture's is the user's own — so the save is refused at the toggle,
  // in the server's words, with the way out beside it (`SPEC.md`, "Agent
  // profiles"; ADR 0051). Nothing was created.
  const autoLaunch = editor.getByRole("checkbox", {
    name: /Let the dispatcher launch this profile/,
  });
  await expect(
    editor.getByText(
      /auto_launch requires this backend's agent credential at global or project scope/,
    ),
  ).toBeVisible();
  await expect(autoLaunch).toBeChecked();
  expect(
    (await api.get<Profile[]>(`/projects/${project.id}/profiles`)).some(
      (candidate) => candidate.name === "reviewer-2",
    ),
  ).toBe(false);

  // Unticked, the same template saves as a reviewer a person launches.
  await autoLaunch.uncheck();
  await editor.getByRole("button", { name: "Create profile" }).click();

  const row = page
    .getByRole("row")
    .filter({ has: page.getByText("reviewer-2", { exact: true }) });
  await expect(row).toBeVisible();
  await expect(row.getByText("review", { exact: true })).toBeVisible();

  // The reload is the real check that the ordinary `POST` stored it.
  await page.reload();
  await expect(row).toBeVisible();

  const profiles = await api.get<Profile[]>(`/projects/${project.id}/profiles`);
  const stored = profiles.find((candidate) => candidate.name === "reviewer-2");
  expect(stored?.serves_states).toEqual(["review"]);
  expect(stored?.mcp_tools).toEqual(["list_session_branches"]);
  expect(stored?.kind).toBe("ephemeral");
  expect(stored?.auto_launch).toBe(false);
  expect(stored?.system_prompt).toMatch(
    /^You are a reviewer of this project\./,
  );
  // `is_default` is informational on the template and never applied here:
  // the seeded `claude` is still the project's default.
  expect(stored?.is_default).toBe(false);
  expect(profiles.find((candidate) => candidate.is_default)?.name).toBe(
    "claude",
  );
});

test("an unknown served state or tool is a 400 the editor cannot produce", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await loginViaToken(context, user);

  const base = await defaultProfile(api, project.id);

  function body(overrides: Record<string, unknown>) {
    return {
      name: uniqueName("bad"),
      kind: "conversational",
      backend: base.backend,
      model: base.model,
      system_prompt: base.system_prompt,
      permission_mode: base.permission_mode,
      image: base.image,
      runtime: base.runtime,
      mcp_tools: [],
      secrets: [],
      serves_states: ["ready"],
      partial_messages: true,
      idle_timeout_secs: base.idle_timeout_secs,
      ...overrides,
    };
  }

  const badState = await api.send(
    "POST",
    `/projects/${project.id}/profiles`,
    body({ serves_states: ["nope"] }),
    { allow: [400] },
  );
  expect(badState.status).toBe(400);
  expect((badState.body as { error: string }).error).not.toBe("");

  const badTool = await api.send(
    "POST",
    `/projects/${project.id}/profiles`,
    body({ mcp_tools: ["nope"] }),
    { allow: [400] },
  );
  expect(badTool.status).toBe(400);
  expect((badTool.body as { error: string }).error).not.toBe("");

  // Neither body is reachable from the editor: both fields are closed sets.
  await page.goto(`/projects/${project.id}?tab=profiles&profile=new`);
  const editor = page.getByRole("form", { name: "New profile" });

  const states = editor.getByRole("group").filter({ hasText: "Served states" });
  const queueStates = ["backlog", "ready", "review", "merge"];
  await expect(states.getByRole("checkbox")).toHaveCount(queueStates.length);
  for (const state of queueStates) {
    await expect(
      states.getByRole("checkbox", { name: state, exact: true }),
    ).toBeVisible();
  }

  const tools = editor.getByRole("group").filter({ hasText: "Git tools" });
  await expect(tools.getByRole("checkbox")).toHaveCount(
    PROFILE_GATED_TOOLS.length,
  );
  for (const tool of PROFILE_GATED_TOOLS) {
    await expect(
      tools.getByRole("checkbox", { name: tool, exact: true }),
    ).toBeVisible();
  }
});

test("a shared directory is added, refused twice, cleared and removed", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await loginViaToken(context, user);

  await page.goto(`/projects/${project.id}?tab=shared-dirs`);
  const form = page.getByRole("form", { name: "Add a shared directory" });

  await form.getByLabel("Name").fill("target");
  await form.getByLabel("Container path").fill("/session/work/target");
  await form.getByRole("button", { name: "Add directory" }).click();

  const row = page
    .getByRole("row")
    .filter({ has: page.getByText("/session/work/target", { exact: true }) });
  await expect(row).toBeVisible();

  // The name is taken: a 409 from the orchestrator, shown on the field.
  await form.getByLabel("Name").fill("target");
  await form.getByLabel("Container path").fill("/session/work/target-two");
  await form.getByRole("button", { name: "Add directory" }).click();
  await expect(form.getByText(/already/i)).toBeVisible();

  // `/session/work` is the session's own checkout, so nothing may be mounted
  // over it (`SPEC.md`, "Shared directories"). The form answers with the rule
  // it broke, in the orchestrator's words, before the request.
  await form.getByLabel("Name").fill("bad");
  await form.getByLabel("Container path").fill("/session/work");
  await form.getByRole("button", { name: "Add directory" }).click();
  await expect(
    form.getByText(
      "Container path must not be, or contain, /session/work, /session/home, /session/log or /session/mcp.json",
    ),
  ).toBeVisible();
  await expect(page.getByRole("row")).toHaveCount(2); // header plus `target`

  await row.getByRole("button", { name: "Clear" }).click();
  await confirm(page, "Empty target");
  await expect(row).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);

  await row.getByRole("button", { name: "Remove" }).click();
  await confirm(page, "Remove target");
  await expect(page.getByText("No shared directories yet")).toBeVisible();
  expect(
    await api.get<SharedDir[]>(`/projects/${project.id}/shared-dirs`),
  ).toEqual([]);
});

test("clearing and removing a shared directory wait for the running session to end", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  test.setTimeout(120_000);

  await loginViaToken(context, user);

  await api.post<SharedDir>(`/projects/${project.id}/shared-dirs`, {
    name: "target",
    container_path: "/session/work/target",
  });

  const session = await sessions.launch(api, project.id);
  await waitForSessionState(api, session.id, "running");

  // Straight to the tab, without opening the sessions tab first: the tab reads
  // the project's session list itself rather than relying on another tab
  // having read it (`SPEC.md`, "Frontend", Project page).
  await page.goto(`/projects/${project.id}?tab=shared-dirs`);
  const row = page
    .getByRole("row")
    .filter({ has: page.getByText("/session/work/target", { exact: true }) });
  await expect(row).toBeVisible();

  await expect(page.getByText(/A session is running/)).toBeVisible();
  await expect(row.getByRole("button", { name: "Clear" })).toBeDisabled();
  await expect(row.getByRole("button", { name: "Remove" })).toBeDisabled();
  expect(
    (await api.get<SharedDir[]>(`/projects/${project.id}/shared-dirs`)).length,
  ).toBe(1);

  await api.post<Session>(`/sessions/${session.id}/end`);
  await waitForSessionState(api, session.id, ["done", "failed"]);

  // No reload: the tab's own poll of that list is what has to bring the two
  // actions back, which is the whole point of it reading the list itself.
  const remove = row.getByRole("button", { name: "Remove" });
  await expect(remove).toBeEnabled({ timeout: 60_000 });
  await remove.click();
  await confirm(page, "Remove target");
  await expect(page.getByText("No shared directories yet")).toBeVisible();
});

test("the settings form renames the project and bounds max_attempts", async ({
  page,
  context,
  user,
  api,
  project,
}) => {
  await loginViaToken(context, user);

  const renamed = uniqueName("e2e-renamed");

  await page.goto(`/projects/${project.id}`);
  await page.getByRole("button", { name: "Settings", exact: true }).click();

  const form = page.getByRole("form", { name: "Project settings" });
  await form.getByLabel("Name").fill(renamed);
  await form.getByLabel("Max attempts").fill("5");
  await form.getByRole("button", { name: "Save settings" }).click();

  await expect(form.getByText("Settings saved.")).toBeVisible();
  await expect(page.getByRole("heading", { name: renamed })).toBeVisible();
  await expect(
    projectHeader(page).getByText("5", { exact: true }),
  ).toBeVisible();

  // 1–20 (`SPEC.md`, "Projects"); the form says so and does not submit.
  await form.getByLabel("Max attempts").fill("0");
  await expect(form.getByText("Between 1 and 20.")).toBeVisible();
  await expect(
    form.getByRole("button", { name: "Save settings" }),
  ).toBeDisabled();

  const stored = await api.get<Project>(`/projects/${project.id}`);
  expect(stored.name).toBe(renamed);
  expect(stored.max_attempts).toBe(5);
});

test("a project is deleted once its running session has ended", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  test.setTimeout(120_000);

  await loginViaToken(context, user);

  const name = project.name;
  const directory = join(dataDir(), "projects", project.id);
  expect(existsSync(directory)).toBe(true);

  const session = await sessions.launch(api, project.id);
  await waitForSessionState(api, session.id, "running");

  await page.goto(`/projects/${project.id}`);
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await confirm(page, `Delete project ${project.name}`);

  // 409 while a session is `running` or `creating` (`SPEC.md`, "Projects").
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/projects/${project.id}`));

  await api.post<Session>(`/sessions/${session.id}/end`);
  await waitForSessionState(api, session.id, ["done", "failed"]);

  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await confirm(page, `Delete project ${project.name}`);

  await expect(page).toHaveURL(/\/projects$/);
  await expect(page.getByRole("link", { name, exact: true })).toHaveCount(0);
  await expect(() => {
    expect(existsSync(directory)).toBe(false);
  }).toPass({ timeout: 15_000 });
  const gone = await api.send("GET", `/projects/${project.id}`, undefined, {
    allow: [404],
  });
  expect(gone.status).toBe(404);
});

// `SPEC.md`, "Frontend", Mobile layout: no route scrolls sideways, and a table
// is what used to make one. The remote URL is the widest thing either table
// shows, so the project is cloned from a path far longer than a phone.
test("the projects and sessions tables fit a phone @mobile", async ({
  page,
  context,
  user,
  api,
  sessions,
}, testInfo) => {
  expect(isMobile(testInfo)).toBe(true);
  // The narrowest phone the layout promises, rather than the device's own.
  await page.setViewportSize({ width: 360, height: 780 });

  const path = join(
    reposDir(),
    `a-remote-url-long-enough-to-push-any-phone-sideways-${"x".repeat(80)}-${randomSuffix()}.git`,
  );
  createBareRepoAt(path);
  const project = await createProject(api, {
    name: uniqueName("e2e-phone"),
    remote_url: `file://${path}`,
  });
  const title = "a session whose title is long enough to wrap on a phone";
  const session = await sessions.launch(api, project.id, { title });
  await waitForSessionState(api, session.id, "running");

  await loginViaToken(context, user);

  await page.goto("/projects");
  const row = projectRow(page, project.name);
  await expect(row).toBeVisible();
  // The remote rides under the name below `md` (the first of its two copies;
  // the `Remote` column's is hidden), truncated inside the row.
  await expect(row.getByText(project.remote_url).first()).toBeVisible();
  await expectNoHorizontalOverflow(page);
  // Truncated rather than scrolled: the row itself fits, so the table's
  // scroller is the safety net and not what holds the URL.
  const box = await row.boundingBox();
  expect(box).not.toBeNull();
  expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(
    page.viewportSize()?.width ?? 0,
  );

  await page.goto(`/projects/${project.id}`);
  await expect(
    page.getByRole("link", { name: title, exact: true }),
  ).toBeVisible();
  await expectNoHorizontalOverflow(page);
});
