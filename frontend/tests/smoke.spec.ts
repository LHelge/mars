import { expect, test } from "@playwright/test";

test("an unauthenticated visit lands on the login route", async ({ page }) => {
  await page.goto("/");

  // No orchestrator is needed: without a token `ProtectedRoute` redirects
  // before anything is fetched (SPEC.md, "Frontend", Rules).
  await expect(page).toHaveURL(/\/login$/);
  await expect(page).toHaveTitle("Mars");
  await expect(page.locator("#root")).not.toBeEmpty();
});
