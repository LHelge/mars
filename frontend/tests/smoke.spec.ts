// The first paint, on the desktop and on a phone.
//
// The desktop scenario needs no stack at all: `test` comes from the shared
// fixtures like everywhere else, and it names none of them, so none is created
// (`tests/utils/fixtures.ts`). The phone scenario signs a fresh user in and so
// needs the orchestrator like every other spec.

import { expect, test } from "./utils/fixtures";
import { isMobile, login } from "./utils/test-helpers";

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
