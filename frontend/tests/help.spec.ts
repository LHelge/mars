// `/help` (`SPEC.md`, "Frontend", Help): a `Learn more` link beside a field
// lands on its topic's anchored section, and the page's own table of contents
// and in-prose links move between sections without leaving the page.
//
// "Lands on" is asserted as the reader has it: the URL carries the topic, the
// section's heading is in the viewport and has focus. The page scrolls itself,
// so a heading that is merely somewhere on the page is not enough.

import { expect, test } from "./utils/fixtures";
import { loginViaToken } from "./utils/test-helpers";

test("a field's Learn more link opens its help topic at the anchored section", async ({
  page,
  context,
  user,
}) => {
  await loginViaToken(context, user);

  await page.goto("/projects");
  // The empty list offers the same button as the header.
  await page.getByRole("button", { name: "New project" }).first().click();
  await page
    .getByRole("link", { name: "Learn more about Git credential" })
    .click();

  await expect(page).toHaveURL(/\/help#git-credential$/);
  const heading = page.getByRole("heading", { name: "Git credential" });
  await expect(heading).toBeInViewport();
  await expect(heading).toBeFocused();
});

test("the help page's contents and topic links move between anchored sections", async ({
  page,
  context,
  user,
}) => {
  await loginViaToken(context, user);

  await page.goto("/");
  await page.getByRole("link", { name: "Help", exact: true }).click();
  await expect(page).toHaveURL(/\/help$/);

  const contents = page.getByRole("navigation", { name: "Help topics" });
  await contents.getByRole("link", { name: "Shared directories" }).click();
  await expect(page).toHaveURL(/\/help#shared-directories$/);
  await expect(
    page.getByRole("heading", { name: "Shared directories" }),
  ).toBeInViewport();
  await expect(
    contents.getByRole("link", { name: "Shared directories" }),
  ).toHaveAttribute("aria-current", "location");

  // A `help:` link in one topic's prose goes to another in place.
  await page
    .getByRole("region", { name: "Agent credentials" })
    .getByRole("link", { name: "secrets", exact: true })
    .click();
  await expect(page).toHaveURL(/\/help#secrets$/);
  const secrets = page.getByRole("heading", { name: "Secrets", exact: true });
  await expect(secrets).toBeInViewport();
  await expect(secrets).toBeFocused();
});
