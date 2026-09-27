// The first paint, on the desktop and on a phone.
//
// The desktop scenario needs no stack at all: `test` comes from the shared
// fixtures like everywhere else, and it names none of them, so none is created
// (`tests/utils/fixtures.ts`). The phone scenarios sign a fresh user in and so
// need the orchestrator like every other spec.

import { expect, test } from "./utils/fixtures";
import {
  apiClient,
  createProject,
  createTestUser,
  isMobile,
  login,
  loginViaToken,
  uniqueName,
} from "./utils/test-helpers";

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

  const overflow = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    innerWidth: window.innerWidth,
  }));
  expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.innerWidth);
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
