// Getting a browser signed in: through the form, or straight past it.
//
// A scenario about authentication uses `login`; every other scenario uses
// `loginViaToken`, which installs the pair `POST /test/users` already issued —
// the access token where `src/services/auth.ts` keeps it in `localStorage`, the
// refresh token as the `refresh_token` cookie — so the form is exercised once
// per suite instead of once per test (`SPEC.md`, "Authentication").

import type { Browser, BrowserContext, Page } from "@playwright/test";

import { TOKEN_STORAGE_KEY } from "../../src/services/auth.ts";
import { REFRESH_COOKIE_NAME, type TestUser } from "./api";
import { baseUrl } from "./env";

/** Signs in through the login form and waits for the dashboard. */
export async function login(
  page: Page,
  username: string,
  password: string,
): Promise<void> {
  await page.goto("/login");
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Sign in" }).click();
  await page.waitForURL("/");
}

/**
 * Seeds `context` with an already signed-in user, without the form.
 *
 * The init script runs before any page script, so the auth module reads the
 * token at import time exactly as it would after a reload.
 */
export async function loginViaToken(
  context: BrowserContext,
  user: TestUser,
): Promise<void> {
  const origin = new URL(baseUrl());

  await context.addCookies([
    {
      name: REFRESH_COOKIE_NAME,
      value: user.refresh_cookie,
      domain: origin.hostname,
      path: "/",
      httpOnly: true,
      sameSite: "Lax",
    },
  ]);

  await context.addInitScript(
    ([key, token]) => {
      try {
        window.localStorage.setItem(key, token);
      } catch {
        // Storage is available in the test browser; if it ever is not, the
        // scenario fails on the redirect to /login, which says more.
      }
    },
    [TOKEN_STORAGE_KEY, user.access_token] as const,
  );
}

/**
 * A fresh context seeded with `user`, and its page — the second browser the
 * live-update scenarios watch while the first one acts.
 */
export async function newLoggedInPage(
  browser: Browser,
  user: TestUser,
): Promise<Page> {
  const context = await browser.newContext({ baseURL: baseUrl() });
  await loginViaToken(context, user);
  return context.newPage();
}
