// The first paint, on the desktop and on a phone, and every route on a phone.
//
// The desktop scenario needs no stack at all: `test` comes from the shared
// fixtures like everywhere else, and it names none of them, so none is created
// (`tests/utils/fixtures.ts`). The phone scenarios sign a fresh user in and so
// need the orchestrator like every other spec.

import type { Page } from "@playwright/test";

import { PROJECT_TABS } from "../src/pages/project/tabs";
import type { Profile, SharedDir } from "../src/types";
import { expect, test } from "./utils/fixtures";
import {
  apiClient,
  createProject,
  createTask,
  createTestUser,
  expectNoHorizontalOverflow,
  FAKE_AGENT_CREDENTIAL,
  isMobile,
  login,
  loginViaToken,
  seedAgentCredential,
  setProjectSecret,
  turnOffAutoLaunch,
  uniqueName,
  waitForSessionState,
  type Api,
} from "./utils/test-helpers";

/**
 * The project-scope secrets the walkthrough stores, deleted after it: `/secrets`
 * lists every project's project-scope rows, so one left behind would be in
 * front of `secrets.spec.ts` (as in `schedules.spec.ts`).
 */
const secrets: { client: Api; id: string }[] = [];

test.afterEach(async () => {
  for (const entry of secrets.splice(0, secrets.length)) {
    await entry.client.delete(`/secrets/${entry.id}`, undefined, {
      allow: [403, 404],
    });
  }
});

test("an unauthenticated visit lands on the login route", async ({ page }) => {
  await page.goto("/");

  // No orchestrator is needed: without a token `ProtectedRoute` redirects
  // before anything is fetched (SPEC.md, "Frontend", Rules).
  await expect(page).toHaveURL(/\/login$/);
  await expect(page).toHaveTitle("Mars");
  await expect(page.locator("#root")).not.toBeEmpty();
});

// `SPEC.md`, "Frontend", Mobile layout: one layout for every width, and at a
// phone's width nothing in it is wider than the screen.
test("the console loads on a phone @mobile", async ({
  page,
  user,
}, testInfo) => {
  // The tag is what put this scenario on the phone project and nowhere else.
  expect(isMobile(testInfo)).toBe(true);

  await login(page, user.username, user.password);
  await expect(page.getByRole("heading", { name: "Dashboard" })).toBeVisible();

  await expectNoHorizontalOverflow(page);
});

// `SPEC.md`, "Frontend", Mobile layout: below `sm` the top nav is icons, each
// a 44 px touch box keeping its label as its name, and the project tabs wrap.
// An administrator is the widest case, with Admin as the fifth entry.
test("the top nav and project tabs fit a phone @mobile", async ({
  page,
  context,
  request,
  repo,
}, testInfo) => {
  expect(isMobile(testInfo)).toBe(true);

  const admin = await createTestUser(request, { prefix: "smoke", admin: true });
  const project = await createProject(apiClient(request, admin.access_token), {
    name: uniqueName("smoke"),
    remote_url: repo.url,
  });
  await loginViaToken(context, admin);

  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Dashboard" })).toBeVisible();
  const width = page.viewportSize()?.width ?? 0;
  expect(width).toBeGreaterThan(0);

  function withinViewport(
    box: {
      x: number;
      width: number;
    } | null,
  ): void {
    expect(box).not.toBeNull();
    expect(box?.x ?? -1).toBeGreaterThanOrEqual(0);
    expect((box?.x ?? width) + (box?.width ?? 0)).toBeLessThanOrEqual(width);
  }

  const nav = page.getByRole("navigation", { name: "Main" });
  await expect(nav.getByRole("link")).toHaveCount(5);
  for (const name of [
    "Dashboard",
    "Projects",
    "Secrets",
    "Settings",
    "Admin",
  ]) {
    const link = nav.getByRole("link", { name, exact: true });
    // Icon-only: the label is the accessible name and not on screen.
    await expect(link.getByText(name, { exact: true })).toHaveClass(
      /max-sm:sr-only/,
    );
    const box = await link.boundingBox();
    expect(box?.width ?? 0).toBeGreaterThanOrEqual(44);
    withinViewport(box);
  }
  withinViewport(
    await page.getByRole("link", { name: "Help", exact: true }).boundingBox(),
  );
  withinViewport(
    await page.getByRole("button", { name: "Log out" }).boundingBox(),
  );

  // Neither the nav nor the header row around it scrolls sideways.
  const scroll = await nav.evaluate((el) => ({
    nav: el.scrollWidth - el.clientWidth,
    row: el.parentElement
      ? el.parentElement.scrollWidth - el.parentElement.clientWidth
      : -1,
  }));
  expect(scroll).toEqual({ nav: 0, row: 0 });

  await page.goto(`/projects/${project.id}`);
  const tabs = page.getByRole("navigation", { name: "Project sections" });
  await expect(tabs.getByRole("link", { name: "Sessions" })).toHaveAttribute(
    "aria-current",
    "page",
  );
  const tabLinks = await tabs.getByRole("link").all();
  expect(tabLinks).toHaveLength(7);
  for (const tab of tabLinks) {
    withinViewport(await tab.boundingBox());
  }
  expect(await tabs.evaluate((el) => el.scrollWidth - el.clientWidth)).toBe(0);
});

/** No spinner left in the page's `main`: the check is of the view, not of its loading state. */
async function settled(page: Page): Promise<void> {
  await expect(
    page.locator("main [role=status]").filter({ hasText: /^Loading/ }),
  ).toHaveCount(0);
}

// `SPEC.md`, "Frontend", Mobile layout: every route of "Routes" at the
// narrowest phone the layout is written for, with a project that has one of
// everything its tabs list, and on each one the document does not scroll
// sideways and no box in its `main` reaches past the screen's right edge. The
// `mobile` project is a Pixel 7, 412 px wide; the viewport is 360 × 740 here,
// which keeps the device's touch and coarse pointer. Names are long on
// purpose, since a name is what a narrow column fails on.
test.describe(() => {
  test.use({ viewport: { width: 360, height: 740 } });

  test("every route fits a phone without horizontal scroll @mobile", async ({
    page,
    context,
    request,
    repo,
    sessions,
  }, testInfo) => {
    expect(isMobile(testInfo)).toBe(true);
    // A container start and a terminal attach on top of some twenty visits.
    test.setTimeout(240_000);

    // ---- the auth pages, signed out ----
    await page.goto("/login");
    await expect(page.getByRole("button", { name: "Sign in" })).toBeVisible();
    await expectNoHorizontalOverflow(page, "/login");
    await page.goto("/forgot-password");
    await expect(
      page.getByRole("heading", { name: "Forgot your password?" }),
    ).toBeVisible();
    await expectNoHorizontalOverflow(page, "/forgot-password");
    await page.goto("/reset-password/x");
    await expect(
      page.getByRole("heading", { name: "Choose a new password" }),
    ).toBeVisible();
    await expectNoHorizontalOverflow(page, "/reset-password/:token");
    await page.goto("/invite/x");
    await expect(
      page.getByRole("link", { name: "Back to sign in" }),
    ).toBeVisible();
    await expectNoHorizontalOverflow(page, "/invite/:token");

    // ---- an administrator with a project that has one of everything ----
    const admin = await createTestUser(request, {
      prefix: "smoke",
      admin: true,
    });
    const api = apiClient(request, admin.access_token);
    await seedAgentCredential(api);
    const project = await createProject(api, {
      name: uniqueName("smoke-a-project-name-long-enough-to-wrap"),
      remote_url: repo.url,
    });
    // The scheduled profile needs a credential an unattended launch can use,
    // and the suite's rule for one is the seeded roles off first
    // (`tests/README.md`, "Automation and the seeded roles").
    sessions.sweep(api, project.id);
    await turnOffAutoLaunch(api, project.id);
    const credential = await setProjectSecret(
      api,
      project.id,
      "CLAUDE_CODE_OAUTH_TOKEN",
      FAKE_AGENT_CREDENTIAL,
    );
    secrets.push({ client: api, id: credential.id });
    const secret = await setProjectSecret(
      api,
      project.id,
      "A_DEPLOY_KEY_WITH_A_NAME_LONGER_THAN_A_PHONE_COLUMN",
      "fake-deploy-key-for-tests",
    );
    secrets.push({ client: api, id: secret.id });
    const profile = await api.post<Profile>(
      `/projects/${project.id}/profiles`,
      {
        name: "a-scheduled-scanner-with-a-long-profile-name",
        kind: "ephemeral",
        // 04:00 UTC on a leap day: never due during a run (`schedules.spec.ts`).
        schedule_cron: "0 4 29 2 *",
        schedule_prompt: "List the files at the repository root and stop.",
      },
    );
    const sharedDir = await api.post<SharedDir>(
      `/projects/${project.id}/shared-dirs`,
      {
        name: "a-shared-build-cache-directory",
        // Outside the work clone: a mount point made inside it belongs to
        // the container's root, and `test:e2e:down` cannot remove it.
        container_path: "/opt/cache/a-long-container-path-for-a-phone",
      },
    );
    const task = await createTask(api, project.id, {
      title:
        "A task whose title is long enough to wrap twice on a phone-width card",
    });
    const title = "a session whose title is long enough to wrap on a phone";
    const session = await sessions.launch(api, project.id, {
      title,
      message: "hello stub",
    });
    await loginViaToken(context, admin);

    // ---- the signed-in routes ----
    await page.goto("/");
    await expect(
      page.getByRole("heading", { name: "Dashboard" }),
    ).toBeVisible();
    await settled(page);
    await expectNoHorizontalOverflow(page, "/");

    await page.goto("/projects");
    await expect(page.getByText(project.name, { exact: true })).toBeVisible();
    await expectNoHorizontalOverflow(page, "/projects");

    // Each tab with the one thing it lists on screen.
    const listed: Record<string, string> = {
      sessions: title,
      board: task.title,
      profiles: profile.name,
      "shared-dirs": sharedDir.name,
      states: "needs_human",
      secrets: secret.name,
    };
    for (const tab of PROJECT_TABS) {
      await page.goto(`/projects/${project.id}?tab=${tab.value}`);
      await expect(
        page
          .getByRole("navigation", { name: "Project sections" })
          .getByRole("link", { name: tab.label, exact: true }),
      ).toHaveAttribute("aria-current", "page");
      const item = listed[tab.value];
      if (item !== undefined) {
        await expect(
          page.getByText(item, { exact: true }).first(),
        ).toBeVisible();
      }
      await settled(page);
      await expectNoHorizontalOverflow(page, `/projects/:id?tab=${tab.value}`);
    }

    await page.goto(`/projects/${project.id}/tasks/${String(task.number)}`);
    const drawer = page.getByRole("dialog", {
      name: `Task #${String(task.number)}`,
    });
    await expect(drawer).toBeVisible();
    await expect(drawer.getByText(task.title).first()).toBeVisible();
    await settled(page);
    await expectNoHorizontalOverflow(page, "/projects/:id/tasks/:number");

    // The session: folded, unfolded, then with the side panel's sheet open on
    // each tab, the terminal attached to the running stub container.
    await waitForSessionState(api, session.id, "running");
    await page.goto(`/sessions/${session.id}`);
    const header = page.locator("main header").first();
    await expect(header.getByText("running", { exact: true })).toBeVisible({
      timeout: 90_000,
    });
    await expectNoHorizontalOverflow(page, "/sessions/:id");
    await header.getByRole("button", { name: "Details" }).click();
    await expectNoHorizontalOverflow(page, "/sessions/:id with Details open");
    await header.getByRole("button", { name: "Panels" }).click();
    const sheet = page.getByRole("dialog", { name: "Session panels" });
    await expect(sheet).toBeVisible();
    for (const name of ["Changes", "Tasks", "Terminal"]) {
      await sheet.getByRole("tab", { name }).click();
      await expect(
        sheet.getByRole("tab", { name, selected: true }),
      ).toBeVisible();
      if (name === "Terminal") {
        // The login shell's first prompt says the exec has attached.
        await expect(sheet.locator(".xterm-rows")).toContainText("@", {
          timeout: 30_000,
        });
      }
      await settled(page);
      await expectNoHorizontalOverflow(page, `/sessions/:id, ${name} sheet`);
    }

    await page.goto("/secrets");
    // The project's credential, in the agent credentials table the page opens
    // on; its other secret is on the project's own tab, above.
    await expect(
      page.getByRole("cell", { name: project.name, exact: true }),
    ).toBeVisible();
    await expectNoHorizontalOverflow(page, "/secrets");

    await page.goto("/settings");
    await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
    await settled(page);
    await expectNoHorizontalOverflow(page, "/settings");

    await page.goto("/help");
    await expect(page.getByRole("heading", { name: "Help" })).toBeVisible();
    await expectNoHorizontalOverflow(page, "/help");

    await page.goto("/admin");
    await expect(
      page.getByRole("heading", { name: "Administration" }),
    ).toBeVisible();
    await expect(
      page.locator("main").getByText(admin.username).first(),
    ).toBeVisible();
    await settled(page);
    await expectNoHorizontalOverflow(page, "/admin");
  });
});
