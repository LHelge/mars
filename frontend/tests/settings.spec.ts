// `/settings` (`SPEC.md`, "Frontend", Routes: own password and
// `notify_email`) and the escalation opt-out of "User-facing features", Login
// and invites: "each user can opt out".
//
// The preference is stored, not remembered: the scenario reloads the page and
// re-reads `GET /users/me` through the helper API, because an optimistic
// checkbox that never reached the orchestrator would look identical until one
// of those two happens.

import type { User } from "../src/types";
import { expect, test } from "./utils/fixtures";
import { currentUser, isMobile, loginViaToken } from "./utils/test-helpers";

/** The opt-out checkbox, found by the sentence beside it. */
const NOTIFY_LABEL = /Email me when a task I am assigned to/;

test("the escalation opt-out is saved, survives a reload and is what the API reports", async ({
  page,
  context,
  user,
  api,
}) => {
  await loginViaToken(context, user);

  // A fresh account is opted in; the scenario is about turning it off.
  expect((await currentUser(api)).notify_email).toBe(true);

  await page.goto("/settings");
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();

  // The account section shows who this is, from `GET /users/me`.
  const account = page.getByRole("main");
  await expect(account.getByText(user.username, { exact: true })).toBeVisible();
  await expect(account.getByText(user.email, { exact: true })).toBeVisible();

  const notify = page.getByLabel(NOTIFY_LABEL);
  await expect(notify).toBeChecked();

  await notify.click();
  await expect(page.getByText("Preferences saved")).toBeVisible();
  await expect(notify).not.toBeChecked();

  await page.reload();
  await expect(page.getByLabel(NOTIFY_LABEL)).not.toBeChecked();

  const me = await currentUser(api);
  expect(me.notify_email).toBe(false);

  // And back on, so the toggle is shown to be a toggle and not a one-way door.
  await page.getByLabel(NOTIFY_LABEL).click();
  await expect(page.getByText("Preferences saved")).toBeVisible();
  await expect
    .poll(async () => (await api.get<User>("/users/me")).notify_email, {
      message: "PATCH /users/me to have turned the preference back on",
    })
    .toBe(true);
});

test("the settings page carries the password form and no administration", async ({
  page,
  context,
  user,
}) => {
  await loginViaToken(context, user);

  await page.goto("/settings");

  await expect(page.getByRole("heading", { name: "Password" })).toBeVisible();
  await expect(page.getByLabel("Current password")).toBeVisible();
  // A member's role is shown as such, and the admin nav entry is not there
  // (`SPEC.md`, "Frontend": the flag is a UI hint over the current user).
  await expect(page.getByText("Member", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("navigation", { name: "Main" }).getByRole("link", {
      name: "Admin",
    }),
  ).toHaveCount(0);
});

// `SPEC.md`, "Frontend", "Mobile layout": on a coarse pointer a text control
// is 16 px, so the phone does not zoom into the field it focuses, and every
// button is a 44 px target. The Pixel 7 of the `mobile` project is a coarse
// pointer, which is what `pointer-coarse:` answers to.
test("the password form is usable on a phone @mobile", async ({
  page,
  context,
  user,
}, testInfo) => {
  expect(isMobile(testInfo)).toBe(true);
  await loginViaToken(context, user);

  await page.goto("/settings");
  await expect(page.getByRole("heading", { name: "Password" })).toBeVisible();

  const current = page.getByLabel("Current password");
  await current.focus();
  await expect(current).toBeFocused();
  expect(
    await current.evaluate((node) => getComputedStyle(node).fontSize),
  ).toBe("16px");

  const buttons = page.getByRole("button");
  await expect(buttons.filter({ hasText: "Change password" })).toBeVisible();
  let measured = 0;
  for (const button of await buttons.all()) {
    if (!(await button.isVisible())) continue;
    const box = await button.boundingBox();
    const name = (await button.textContent()) ?? "";
    expect(box, `${name} has a box`).not.toBeNull();
    expect(
      box?.height ?? 0,
      `${name} is at least 44 px tall`,
    ).toBeGreaterThanOrEqual(44);
    measured += 1;
  }
  // The form's own button and the header's `Log out` at the least.
  expect(measured).toBeGreaterThanOrEqual(2);
});
