// Getting a browser signed in: through the form, or straight past it.
//
// A scenario about authentication uses `login`; every other scenario uses
// `loginViaToken`, which installs the pair `POST /test/users` already issued —
// the access token where `src/services/auth.ts` keeps it in `localStorage`, the
// refresh token as the `refresh_token` cookie — so the form is exercised once
// per suite instead of once per test (`SPEC.md`, "Authentication").

import type { Browser, BrowserContext, Page, TestInfo } from "@playwright/test";

import { TOKEN_STORAGE_KEY } from "../../src/services/auth.ts";
import { REFRESH_COOKIE_NAME, type TestUser } from "./api";
import { baseUrl } from "./env";

/**
 * Whether this scenario is running on the phone project — the one a title
 * tagged `@mobile` selects (`playwright.config.ts`; tests/README.md, "Running
 * it") — for a scenario whose steps differ on a phone.
 */
export function isMobile(testInfo: TestInfo): boolean {
  return testInfo.project.name === "mobile";
}

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

/** The page-global `dropConnection` installs and calls. */
const DROP_HOOK = "__marsDropSockets";

/**
 * Makes the page's WebSockets closable from the test, for [`dropConnection`].
 *
 * `BrowserContext.setOffline` alone is not enough: Chromium's offline
 * emulation fails new requests but leaves an already established WebSocket
 * open, so a session socket survives the outage and the client never runs its
 * reconnect path. What does close it is the page closing it — so every socket
 * whose URL contains `match` is tracked here, behind a subclass of the real
 * `WebSocket`, leaving framing, binary frames and every other behaviour the
 * client relies on untouched.
 *
 * The init script has to be in place before the page that opens the socket
 * loads, so this is called on the context, beside `loginViaToken`, and before
 * the first navigation.
 */
export async function armSocketDrop(
  context: BrowserContext,
  match = "/ws/",
): Promise<void> {
  await context.addInitScript(
    ([hook, needle]) => {
      const open = new Set<WebSocket>();
      const Native = window.WebSocket;

      class Tracked extends Native {
        constructor(url: string | URL, protocols?: string | string[]) {
          super(url, protocols);
          if (!String(url).includes(needle)) return;
          open.add(this);
          this.addEventListener("close", () => open.delete(this));
        }
      }

      window.WebSocket = Tracked;
      Object.defineProperty(window, hook, {
        value: () => {
          const count = open.size;
          // 4900 is in the private range: the client treats any non-1008
          // close the same way, and a distinct code names the cause in a log.
          for (const socket of open) socket.close(4900, "dropped by the test");
          open.clear();
          return count;
        },
      });
    },
    [DROP_HOOK, match] as const,
  );
}

/**
 * Takes the page's network away for `offlineMs`, closing the sockets it holds,
 * and gives it back — the forced disconnect the reconnect scenarios need.
 *
 * Both halves matter. The close is what makes the client notice; being offline
 * across it is what keeps it noticing, because the reconnect refreshes the
 * access token first (`SPEC.md`, "Authentication") and that request has to
 * fail for a while, or the socket is back before a reader — or an assertion —
 * ever sees `reconnecting`.
 *
 * Nothing on the server side is touched: the session goes on running and the
 * events it commits meanwhile are exactly what the reopened socket replays
 * from `?after=<seq>` (`SPEC.md`, "WebSocket: session stream").
 *
 * Requires [`armSocketDrop`] on the context before the page loaded.
 */
export async function dropConnection(
  page: Page,
  offlineMs = 3000,
): Promise<number> {
  const context = page.context();
  await context.setOffline(true);
  const dropped = await page.evaluate((hook) => {
    const drop = (window as unknown as Record<string, unknown>)[hook];
    if (typeof drop !== "function") {
      throw new Error(
        "dropConnection: armSocketDrop() was not installed before this page loaded",
      );
    }
    return (drop as () => number)();
  }, DROP_HOOK);
  // The one deliberate sleep in the suite: `offlineMs` *is* the outage. There
  // is nothing to wait for here — the point is that for this long every request
  // the page makes, the reconnect's token refresh among them, fails.
  await page.waitForTimeout(offlineMs);
  await context.setOffline(false);
  return dropped;
}

/**
 * Closes the page's tracked sockets with the network left up, and returns how
 * many were closed.
 *
 * This is the other half of [`dropConnection`]. That one models an outage, and
 * an outage is precisely when the reconnect's token refresh cannot succeed:
 * `POST /auth/refresh` fails with a network error, the socket comes back on a
 * scheduled retry carrying the token it already had, and a scenario about the
 * refresh would be asserting nothing. A close with the network up runs the
 * documented path end to end instead — refresh, then reopen with a fresh token
 * and the last cursor (`SPEC.md`, "Authentication").
 *
 * Requires [`armSocketDrop`] on the context before the page loaded.
 */
export async function closeSockets(page: Page): Promise<number> {
  return page.evaluate((hook) => {
    const drop = (window as unknown as Record<string, unknown>)[hook];
    if (typeof drop !== "function") {
      throw new Error(
        "closeSockets: armSocketDrop() was not installed before this page loaded",
      );
    }
    return (drop as () => number)();
  }, DROP_HOOK);
}
