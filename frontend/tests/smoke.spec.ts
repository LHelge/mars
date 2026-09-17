import { expect, test } from "@playwright/test";

test("app shell loads", async ({ page }) => {
  await page.goto("/");

  await expect(page).toHaveTitle("Mars");
  await expect(page.locator("#root")).not.toBeEmpty();
  // No orchestrator is needed for the skeleton: the placeholder page renders the
  // failed /api/health fetch instead of throwing.
  await expect(page.getByText("orchestrator unreachable")).toBeVisible();
});
