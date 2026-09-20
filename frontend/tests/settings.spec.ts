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
import { currentUser, loginViaToken } from "./utils/test-helpers";

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
