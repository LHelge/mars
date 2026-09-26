// "Login and invites" end to end (`SPEC.md`, "User-facing features", Login and
// invites; "Authentication"; "Auth (`/api/auth`)"; "Users (`/api/users`)";
// "Frontend", Routes and Copy links).
//
// Everything here is driven through the browser against the real orchestrator:
// the seeded administrator's forced first change, ordinary login and logout,
// the return destination a guard stashed, an invitation whose link is read back
// out of the orchestrator's log (ADR 0026), a revoked invitation, a password
// reset through the same mechanism, and a self-service change that keeps this
// browser signed in while every other session of the account loses its pair.
//
// Every scenario but the first makes its own users through `createTestUser`.
// The first is the only one allowed near `admin`/`changeme`, and it consumes
// that account: a later `npm run test:e2e` against the same stack finds the
// seeded password already changed and skips it (see its own comment).
//
// Login throttling (429 after 10 failures in 15 minutes) is deliberately not
// exercised: the limit is per client address, so tripping it here would poison
// every later scenario in the run. It is covered by the backend integration
// tests. The failures this file does make — and the seeded-account probe on a
// rerun — stay below the limit because `global-setup.ts` clears the throttle
// before every run.

import type { Browser, Page } from "@playwright/test";

import { TOKEN_STORAGE_KEY } from "../src/services/auth";
import type { User } from "../src/types";
import { expect, test } from "./utils/fixtures";
import {
  api,
  apiBaseUrl,
  baseUrl,
  createTestUser,
  DEFAULT_TEST_PASSWORD,
  login,
  logOffset,
  loginViaToken,
  newLoggedInPage,
  randomSuffix,
  readLoggedLink,
} from "./utils/test-helpers";

/** `README.md`, "Start": the account the first migration seeds. */
const SEEDED_USERNAME = "admin";
const SEEDED_PASSWORD = "changeme";

/** Nine characters: one short of the 10–128 rule, so the form refuses it. */
const TOO_SHORT_PASSWORD = "short-pw1";

/** The message `validatePassword` shows for a length outside 10–128. */
const PASSWORD_LENGTH_MESSAGE = "Password must be 10–128 characters";

/** A second obviously fake password, for the flows that change one (rule 3). */
const REPLACEMENT_PASSWORD = "E2e-replacement-5678";

/** The access token the page has stored, or null when it is signed out. */
function storedToken(page: Page): Promise<string | null> {
  return page.evaluate(
    (key) => window.localStorage.getItem(key),
    TOKEN_STORAGE_KEY,
  );
}

/** Fills and submits the sign-in form, without waiting for where it lands. */
async function submitLogin(
  page: Page,
  username: string,
  password: string,
): Promise<void> {
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Sign in" }).click();
}

/** The `PasswordChangeForm` of `/change-password` and `/settings`. */
async function submitPasswordChange(
  page: Page,
  current: string,
  next: string,
): Promise<void> {
  await page.getByLabel("Current password").fill(current);
  await page.getByLabel("New password", { exact: true }).fill(next);
  await page.getByLabel("Repeat new password").fill(next);
  await page.getByRole("button", { name: "Change password" }).click();
}

/** The "Invitations" half of `/admin`, scoped away from the users table. */
function invitesSection(page: Page) {
  return page
    .locator("section")
    .filter({ has: page.getByRole("heading", { name: "Invitations" }) });
}

/**
 * The invitation's row in that table. The panel's success banner names the
 * address too, so the row — not the text — is what "is listed" means here.
 */
function inviteRow(page: Page, email: string) {
  return invitesSection(page).locator("tr").filter({ hasText: email });
}

/** A signed-out browser of its own, for a link that must be opened cold. */
async function newAnonymousPage(browser: Browser): Promise<Page> {
  const context = await browser.newContext({ baseURL: baseUrl() });
  return context.newPage();
}

test.describe("the seeded administrator", () => {
  // The one block that touches `admin`, and the one that has to run before
  // anything else could: `workers: 1` and alphabetical file order put
  // `auth.spec.ts` first, and serial mode keeps the block itself in order.
  test.describe.configure({ mode: "serial" });

  test("must change password before anything else", async (
    { page, request },
    testInfo,
  ) => {
    // A rerun against a stack that is already up finds the seeded password
    // spent, because this very test changed it. `npm run test:e2e:up` starts
    // from an empty database and brings it back.
    const probe = await request.post(`${apiBaseUrl()}/api/auth/login`, {
      data: { username: SEEDED_USERNAME, password: SEEDED_PASSWORD },
      failOnStatusCode: false,
    });
    const fresh = probe.status() === 200;

    // Only the *first* attempt may skip. A retry (CI runs with `retries: 1`)
    // that finds the seeded password spent found it spent because the attempt
    // being retried changed it and then failed somewhere after: skipping there
    // would report a real failure as a pass. The first attempt cannot be in
    // that position — had the password been spent before it, it would have
    // skipped and there would be no retry.
    test.skip(
      !fresh && testInfo.retry === 0,
      "admin/changeme no longer signs in: this stack is not fresh, run `npm run test:e2e:up` first",
    );
    if (!fresh) {
      throw new Error(
        "the seeded password was already changed by the attempt being retried: " +
          "this is a failure of that attempt, not a stack that was never fresh",
      );
    }

    await page.goto("/login");
    await submitLogin(page, SEEDED_USERNAME, SEEDED_PASSWORD);

    // `must_change_password` on the login response sends the browser here.
    await page.waitForURL("/change-password");
    await expect(
      page.getByText("You must change your password before continuing."),
    ).toBeVisible();

    // Every gated route bounces back while the flag is set.
    await page.goto("/projects");
    await expect(page).toHaveURL(/\/change-password$/);

    // The guard stashes the blocked destination and the change returns to it
    // (`SPEC.md`, "Frontend", Copy links), so the bounce above would end on
    // `/projects`. Coming from the dashboard instead is what makes "lands on
    // `/`" a statement about the change rather than about `/projects`. A
    // second `goto("/change-password")` would not do: the URL is unchanged, so
    // the browser reloads and `history.state` — the stashed destination —
    // survives.
    await page.goto("/");
    await expect(page).toHaveURL(/\/change-password$/);

    await submitPasswordChange(page, SEEDED_PASSWORD, TOO_SHORT_PASSWORD);
    await expect(page.getByText(PASSWORD_LENGTH_MESSAGE)).toBeVisible();
    await expect(page).toHaveURL(/\/change-password$/);

    await submitPasswordChange(page, SEEDED_PASSWORD, DEFAULT_TEST_PASSWORD);

    await page.waitForURL("/");
    await expect(
      page.getByRole("heading", { name: "Dashboard" }),
    ).toBeVisible();
    // Still signed in: no login form anywhere on the page.
    await expect(page.getByRole("button", { name: "Sign in" })).toHaveCount(0);

    const token = await storedToken(page);
    expect(token).not.toBeNull();
    const me = await api(request, token ?? "").get<User>("/users/me");
    expect(me.username).toBe(SEEDED_USERNAME);
    expect(me.must_change_password).toBe(false);
  });
});

test("login rejects a wrong password", async ({ page, user }) => {

  await page.goto("/login");
  await submitLogin(page, user.username, "not-the-password");

  await expect(page.getByRole("alert")).toHaveText(
    "Invalid username or password",
  );
  await expect(page).toHaveURL(/\/login$/);
  expect(await storedToken(page)).toBeNull();
});

test("login and logout", async ({ page, user }) => {

  await login(page, user.username, user.password);
  await expect(page.getByRole("heading", { name: "Dashboard" })).toBeVisible();

  await page.getByRole("button", { name: "Log out" }).click();
  await page.waitForURL("/login");
  expect(await storedToken(page)).toBeNull();

  await page.goto("/");
  await expect(page).toHaveURL(/\/login$/);
});

test("a deep link is preserved through login", async ({ page, user }) => {

  // `ProtectedRoute` stashes the blocked destination in router state
  // (`SPEC.md`, "Frontend", Copy links).
  await page.goto("/secrets");
  await expect(page).toHaveURL(/\/login$/);

  await submitLogin(page, user.username, user.password);

  await page.waitForURL("/secrets");
  await expect(
    page.getByRole("heading", { name: "Secrets", exact: true }),
  ).toBeVisible();
});

test("an admin invites a user who accepts via the logged link", async ({
  page,
  context,
  browser,
  request,
}) => {
  const admin = await createTestUser(request, {
    prefix: "inviter",
    admin: true,
  });
  await loginViaToken(context, admin);

  const email = `invitee-${randomSuffix()}@example.test`;
  await page.goto("/admin");

  const offset = logOffset();
  await invitesSection(page).getByLabel("Email").fill(email);
  await page.getByRole("button", { name: "Send invitation" }).click();
  await expect(inviteRow(page, email)).toBeVisible();

  const link = await readLoggedLink("invite", email, offset);

  // A cold browser: the invitee is not the admin who sent the link.
  const invitee = await newAnonymousPage(browser);
  await invitee.goto(link);
  await expect(invitee.getByLabel("Invited email")).toHaveValue(email);

  const username = `e2e-accepted-${randomSuffix()}`;
  await invitee.getByLabel("Username").fill(username);
  await invitee
    .getByLabel("Password", { exact: true })
    .fill(DEFAULT_TEST_PASSWORD);
  await invitee.getByLabel("Repeat password").fill(DEFAULT_TEST_PASSWORD);
  await invitee.getByRole("button", { name: "Create account" }).click();

  await invitee.waitForURL("/");
  await expect(
    invitee.getByRole("heading", { name: "Dashboard" }),
  ).toBeVisible();
  await expect(invitee.getByText(username)).toBeVisible();

  // The invitation is spent, so the admin's list no longer carries it.
  await page.reload();
  await expect(inviteRow(page, email)).toHaveCount(0);

  // `GET /auth/invite/{token}` answers 400 for a used token; the page turns
  // that into its one dead-end message and offers no form.
  await invitee.goto(link);
  await expect(invitee.getByRole("alert")).toContainText(
    "invalid, has expired or was already used",
  );
  await expect(
    invitee.getByRole("button", { name: "Create account" }),
  ).toHaveCount(0);
});

test("a revoked invitation cannot be accepted", async ({
  page,
  context,
  browser,
  request,
}) => {
  const admin = await createTestUser(request, {
    prefix: "revoker",
    admin: true,
  });
  await loginViaToken(context, admin);

  const email = `invitee-${randomSuffix()}@example.test`;
  await page.goto("/admin");

  const offset = logOffset();
  await invitesSection(page).getByLabel("Email").fill(email);
  await page.getByRole("button", { name: "Send invitation" }).click();
  await expect(inviteRow(page, email)).toBeVisible();

  const link = await readLoggedLink("invite", email, offset);

  // The panel confirms a revoke inline, under the row it belongs to.
  await inviteRow(page, email)
    .getByRole("button", { name: "Revoke", exact: true })
    .click();
  await page
    .getByRole("button", {
      name: `Revoke the invitation for ${email}`,
      exact: true,
    })
    .click();
  await expect(inviteRow(page, email)).toHaveCount(0);

  const invitee = await newAnonymousPage(browser);
  await invitee.goto(link);

  await expect(invitee.getByRole("alert")).toContainText(
    "invalid, has expired or was already used",
  );
  await expect(invitee.getByLabel("Invited email")).toHaveCount(0);
  await expect(
    invitee.getByRole("button", { name: "Create account" }),
  ).toHaveCount(0);
});

test("a password reset through the logged link replaces the password", async ({
  page,
  user,
}) => {

  await page.goto("/forgot-password");
  await page.getByLabel("Username or email").fill(user.username);

  const offset = logOffset();
  await page.getByRole("button", { name: "Send reset link" }).click();

  // 204 whatever the identifier was, so the page says the same thing either way.
  await expect(page.getByRole("status")).toHaveText(
    "If that account exists, a reset link has been sent.",
  );

  const link = await readLoggedLink("reset-password", user.email, offset);

  await page.goto(link);
  await page
    .getByLabel("New password", { exact: true })
    .fill(REPLACEMENT_PASSWORD);
  await page.getByLabel("Repeat new password").fill(REPLACEMENT_PASSWORD);
  await page.getByRole("button", { name: "Set password" }).click();

  // A reset does not sign anyone in (`SPEC.md`, "Authentication"): the page
  // ends on a link back to the form.
  await expect(page.getByRole("status")).toHaveText(
    "Password updated. Sign in with your new password.",
  );
  expect(await storedToken(page)).toBeNull();

  await page.getByRole("link", { name: "Back to sign in" }).click();
  await page.waitForURL("/login");

  await submitLogin(page, user.username, user.password);
  await expect(page.getByRole("alert")).toHaveText(
    "Invalid username or password",
  );

  await submitLogin(page, user.username, REPLACEMENT_PASSWORD);
  await page.waitForURL("/");
  await expect(page.getByRole("heading", { name: "Dashboard" })).toBeVisible();
});

test("the settings page changes the password and keeps only this session", async ({
  page,
  context,
  browser,
  user,
}) => {

  // A second browser holding the pair `POST /test/users` issued, signed in
  // before the change and left open across it.
  const other = await newLoggedInPage(browser, user);
  await other.goto("/");
  await expect(other.getByRole("heading", { name: "Dashboard" })).toBeVisible();

  await loginViaToken(context, user);
  await page.goto("/settings");
  await submitPasswordChange(page, user.password, REPLACEMENT_PASSWORD);

  await expect(
    page.getByText("Password changed. Other sessions of your account"),
  ).toBeVisible();
  // Still here: the change installs the replacement pair rather than signing out.
  await expect(page).toHaveURL(/\/settings$/);

  // A further call from the same page, with the pair the change installed:
  // `PATCH /users/me`. The box is toggled to whatever it is not, so the
  // request is made whichever way the account's default falls.
  const notify = page.getByRole("checkbox");
  const before = await notify.isChecked();
  await notify.click();
  await expect(page.getByText("Preferences saved")).toBeVisible();
  await expect(notify).toBeChecked({ checked: !before });

  // The other browser's access token died with the bumped `auth_version` and
  // its refresh cookie was revoked in the same transaction, so its next load
  // refreshes once, gets 401 and returns to the login form.
  await other.goto("/");
  await other.waitForURL("/login");
  expect(await storedToken(other)).toBeNull();
});
