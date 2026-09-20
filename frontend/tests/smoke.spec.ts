// The one spec that needs no stack at all: `test` comes from the shared
// fixtures like everywhere else, and this file names none of them, so none is
// created (`tests/utils/fixtures.ts`).

import { expect, test } from "./utils/fixtures";

test("an unauthenticated visit lands on the login route", async ({ page }) => {
  await page.goto("/");

  // No orchestrator is needed: without a token `ProtectedRoute` redirects
  // before anything is fetched (SPEC.md, "Frontend", Rules).
  await expect(page).toHaveURL(/\/login$/);
  await expect(page).toHaveTitle("Mars");
  await expect(page.locator("#root")).not.toBeEmpty();
});
