// `/admin` behind `AdminRoute` (`SPEC.md`, "User-facing features", Users
// (admin), "Users (`/api/users`)" and "Frontend", Routes): who may open it,
// promoting and demoting, the refusals that keep an administrator in place,
// and what a deleted user's browser does at its next request.
//
// Every administrator here is made with `createTestUser({ admin: true })`; the
// seeded `admin` account belongs to the authentication spec. The user list is
// the whole database, which the other spec files are filling at the same time,
// so nothing asserts on counts or on rows this file did not create.
//
// The one contract not asserted from a browser is the last-administrator 409
// of `SPEC.md`, "Users": the seeded administrator and every other spec's
// administrators share this database, so a demotion here never is the last
// one. That transaction is covered by the orchestrator suite
// (`orchestrator/tests/users_last_admin_race.rs`,
// `orchestrator/tests/repositories_users.rs`); what a browser can show is the
// other half of the same paragraph — self-demotion is allowed while another
// administrator remains, and self-deletion never is.

import type { Page } from "@playwright/test";

import { expect, test } from "./utils/fixtures";
import {
  api,
  createTestUser,
  DEFAULT_TEST_PASSWORD,
  loginViaToken,
  newLoggedInPage,
  randomSuffix,
} from "./utils/test-helpers";

/** How long a demotion or deletion is given to take effect on the next request. */
const NEXT_REQUEST_MS = 10_000;

const FORBIDDEN = "Administrator access required";

function adminNavEntry(page: Page) {
  return page
    .getByRole("navigation", { name: "Main" })
    .getByRole("link", { name: "Admin" });
}

function userRow(page: Page, username: string) {
  return page.getByRole("row").filter({ hasText: username });
}

test("a member cannot open /admin", async ({ page, context, user }) => {
  await loginViaToken(context, user);

  await page.goto("/admin");

  await expect(page.getByText(FORBIDDEN)).toBeVisible();
  // Not a redirect, and not a user list either: the guard renders in place.
  await expect(page).toHaveURL(/\/admin$/);
  await expect(page.getByRole("heading", { name: "Users" })).toHaveCount(0);
  // The header greets whoever is signed in; the guarded page itself lists
  // nobody at all.
  await expect(page.getByRole("main").getByText(user.username)).toHaveCount(0);
  await expect(page.getByRole("table")).toHaveCount(0);
  await expect(adminNavEntry(page)).toHaveCount(0);
});

test("an administrator sees the sections, promotes a member and demotes them again", async ({
  page,
  context,
  request,
  browser,
}) => {
  // Two browsers, and every step waits on a real request.
  test.slow();

  const admin = await createTestUser(request, { prefix: "admin", admin: true });
  const member = await createTestUser(request, { prefix: "promoted" });
  await loginViaToken(context, admin);

  await page.goto("/admin");
  await expect(
    page.getByRole("heading", { name: "Administration" }),
  ).toBeVisible();
  await expect(page.getByRole("heading", { name: "Users" })).toBeVisible();
  // The invitations half is the auth spec's subject; here it only has to be
  // on the page an administrator opens.
  await expect(
    page.getByRole("heading", { name: "Invitations" }),
  ).toBeVisible();

  const row = userRow(page, member.username);
  await expect(row).toBeVisible();
  await expect(row).toContainText(member.email);

  const flag = row.getByLabel(`Administrator: ${member.username}`);
  await expect(flag).not.toBeChecked();
  // The checkbox is controlled by the query data, so it only moves once the
  // orchestrator has answered: click, then wait for the row to follow.
  await flag.click();
  await expect(flag).toBeChecked();

  // --- the promoted member's own browser ------------------------------------

  const memberPage = await newLoggedInPage(browser, member);
  try {
    await memberPage.goto("/admin");
    await expect(
      memberPage.getByRole("heading", { name: "Administration" }),
    ).toBeVisible({ timeout: NEXT_REQUEST_MS });
    await expect(adminNavEntry(memberPage)).toBeVisible();

    // --- demoted again, and it lands on the next request --------------------

    await flag.click();
    await expect(flag).not.toBeChecked();
    await expect
      .poll(
        async () =>
          (
            await api(request, admin.access_token).get<{ admin: boolean }>(
              `/users/${member.id}`,
            )
          ).admin,
        { message: "the demotion to have reached the orchestrator" },
      )
      .toBe(false);

    // The member's browser still believes it is an administrator: the page it
    // is on was rendered before the demotion and nothing has asked the
    // orchestrator since. The next administrator request is the one that finds
    // out — here, sending an invitation — and its 403 refreshes the current
    // user, which closes the page and takes the nav entry with it (`SPEC.md`,
    // "Authentication": a demotion takes effect on the next request;
    // "Frontend", Rules). The invitation is never created.
    await memberPage
      .getByLabel("Email")
      .fill(`e2e-never-invited-${randomSuffix()}@example.test`);
    await memberPage.getByRole("button", { name: "Send invitation" }).click();

    await expect(memberPage.getByText(FORBIDDEN)).toBeVisible({
      timeout: NEXT_REQUEST_MS,
    });
    await expect(adminNavEntry(memberPage)).toHaveCount(0, {
      timeout: NEXT_REQUEST_MS,
    });

    // A fresh visit is refused as well, this time from the startup read of
    // `GET /users/me` rather than from a 403.
    await memberPage.goto("/admin");
    await expect(memberPage.getByText(FORBIDDEN)).toBeVisible();
    await expect(adminNavEntry(memberPage)).toHaveCount(0);
  } finally {
    // The page first: closing the context alone can wait on a page that
    // still has work in flight.
    await memberPage.close();
    await memberPage.context().close();
  }
});

test("an administrator may step down but never delete itself", async ({
  page,
  context,
  request,
}) => {
  const admin = await createTestUser(request, {
    prefix: "stepping-down",
    admin: true,
  });
  const client = api(request, admin.access_token);
  await loginViaToken(context, admin);

  await page.goto("/admin");
  const own = userRow(page, admin.username);
  await expect(own).toContainText("(you)");

  // Self-deletion is refused by the orchestrator (`SPEC.md`, "Users": 409 for
  // the last administrator or yourself) and the row never offers it.
  const deleteButton = own.getByRole("button", { name: "Delete" });
  await expect(deleteButton).toBeDisabled();
  await expect(
    own.getByTitle("You cannot delete your own account"),
  ).toBeVisible();
  const refused = await client.send("DELETE", `/users/${admin.id}`, undefined, {
    allow: [409],
  });
  expect(refused.status).toBe(409);
  expect(refused.text).toContain("yourself");

  // Stepping down is allowed while other administrators remain — which they
  // do, the seeded one among them. It is confirmed, and takes the page with
  // it: the refetch that follows answers 403 and the guard closes.
  page.on("dialog", (dialog) => {
    void dialog.accept();
  });
  await own.getByLabel(`Administrator: ${admin.username}`).click();

  await expect(page.getByText(FORBIDDEN)).toBeVisible({
    timeout: NEXT_REQUEST_MS,
  });
  await expect(adminNavEntry(page)).toHaveCount(0, {
    timeout: NEXT_REQUEST_MS,
  });
  expect((await client.get<{ admin: boolean }>("/users/me")).admin).toBe(false);
});

test("a deleted user is signed out at its next request and cannot sign in again", async ({
  page,
  context,
  request,
  browser,
}) => {
  const admin = await createTestUser(request, {
    prefix: "deleter",
    admin: true,
  });
  const doomed = await createTestUser(request, { prefix: "deleted" });
  await loginViaToken(context, admin);

  const doomedPage = await newLoggedInPage(browser, doomed);
  try {
    await doomedPage.goto("/settings");
    await expect(
      doomedPage.getByRole("heading", { name: "Settings" }),
    ).toBeVisible();

    await page.goto("/admin");
    page.on("dialog", (dialog) => {
      void dialog.accept();
    });
    const row = userRow(page, doomed.username);
    await expect(row).toBeVisible();
    await row.getByRole("button", { name: "Delete" }).click();
    await expect(userRow(page, doomed.username)).toHaveCount(0);

    // The access token names a user that no longer exists, so the next request
    // is a 401; the one refresh `apiClient` allows is a 401 too, and the
    // failed refresh returns the browser to the login page (`SPEC.md`,
    // "Frontend", Rules).
    await doomedPage.reload();
    await expect(doomedPage).toHaveURL(/\/login$/, {
      timeout: NEXT_REQUEST_MS,
    });

    await doomedPage.getByLabel("Username").fill(doomed.username);
    await doomedPage.getByLabel("Password").fill(DEFAULT_TEST_PASSWORD);
    await doomedPage.getByRole("button", { name: "Sign in" }).click();

    await expect(doomedPage.getByRole("alert")).toBeVisible();
    await expect(doomedPage).toHaveURL(/\/login$/);
  } finally {
    // The page first: closing the context alone can wait on a page that
    // still has work in flight.
    await doomedPage.close();
    await doomedPage.context().close();
  }
});
