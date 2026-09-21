// What a reconnect does to the session *page*, as opposed to the transcript —
// whose "no gaps, no duplicates" sentence belongs to `session-view.spec.ts`.
//
// The question this file settles (77ue3): a reconnect installs auth state, and
// `services/auth.ts#installSession` publishes a whole new state to every
// `useAuth()` subscriber when it does. If that state ever passed through
// `user: null`, or if any route element changed identity with the token, then
// `ProtectedRoute` would fall back to `LoadingState` and `SessionPage` would
// unmount and mount again — discarding the composer's unsent text, resetting
// the side panel to its first tab and handing the operator a fresh terminal
// instead of the documented `Terminal disconnected` with `Reconnect`
// (`SPEC.md`, "Frontend"; `session/TerminalView.tsx`).
//
// It does not: against a real orchestrator the page stays mounted through
// both an outage and a token rotation, so this file is the regression test
// that keeps it that way rather than the reproduction of a bug.
//
// The assertions are therefore about *identity*, not appearance: marks put on
// the mounted DOM nodes themselves, which no remount can carry over, plus the
// request log across the reconnect — a re-bootstrap reloads `GET /users/me`
// (`AuthBootstrap`) and a remounted `SessionRoute` re-reads
// `GET /sessions/{id}`.
//
// Two closes, because the two halves of a reconnect cannot be had at once:
//
//   1. An outage (`dropConnection`), a real disconnect, reconnect and replay
//      with every request in between failing.
//   2. A close with the network up (`closeSockets`), which comes straight back
//      and so exercises the same page through a reconnect that never leaves
//      the transcript behind.
//
// Both halves also assert what ntepg made of the close path: an ordinary close
// is no evidence about the access token — an open stream is not closed because
// its token expired (`SPEC.md`, "Authentication") — so neither half rotates
// anything, and the single-use refresh cookie is spent only on the
// authentication close (1008) that says the credentials are the problem. That
// close cannot be staged from here: `WebSocket.close` refuses every reserved
// code, 1008 among them, and the server sends it only for a revocation, which
// ends in sign-out rather than in a rotation. What an installed pair does to
// this mounted page is therefore asserted where a rotation really happens —
// `src/session/socketTokenRotation.test.ts` for the socket, and the
// self-service password change of `auth.spec.ts` for the whole page.

import type { Locator, Page } from "@playwright/test";

import { expect, test } from "./utils/fixtures";
import {
  armSocketDrop,
  closeSockets,
  dropConnection,
  loginViaToken,
  TRANSCRIPT_SCROLL,
  waitForSessionState,
} from "./utils/test-helpers";

// The upstream this scenario clones: the file the fixture's `Edit` tool
// rewrites.
test.use({ repoFiles: { "src/app.py": 'def main():\n    print("hello")\n' } });

// A container start, a git clone, a replayed turn and two reconnects.
test.setTimeout(180_000);

/** The unsent draft the composer must still hold on the other side. */
const DRAFT = "half-written thought, never sent";

/** Where `services/auth.ts` keeps the access token. */
const TOKEN_KEY = "mars.access_token";

/** The session header band, scoped past the application shell's own header. */
function header(page: Page): Locator {
  return page.locator("main header").first();
}

/** The connection dot's accessible name (`SessionHeader`, `role="status"`). */
function connection(page: Page, status: string): Locator {
  return header(page).getByRole("status", { name: `Connection ${status}` });
}

/** The composer's text area (`Composer`, `aria-label="Message"`). */
function messageBox(page: Page): Locator {
  return page.getByLabel("Message", { exact: true });
}

function terminalTab(page: Page): Locator {
  return page.getByRole("tab", { name: "Terminal" });
}

/** One `/api` call the page made, with the status it was answered with. */
interface Call {
  method: string;
  path: string;
  /** `null` while the answer is outstanding, and for a failed request. */
  status: number | null;
}

/**
 * Records every `/api` call the page makes from now on.
 *
 * A remount leaves no trace in the DOM once it has happened, but it is loud in
 * the request log: `AuthBootstrap` reloads `GET /users/me` whenever the
 * current user is null, and a remounted `SessionRoute` re-reads
 * `GET /sessions/{id}`.
 */
function recordCalls(page: Page): Call[] {
  const calls: Call[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (!path.startsWith("/api/")) return;
    const call: Call = { method: request.method(), path, status: null };
    calls.push(call);
    void request
      .response()
      .then((response) => {
        if (response !== null) call.status = response.status();
      })
      .catch(() => undefined);
  });
  return calls;
}

function countOf(calls: Call[], method: string, path: string): number {
  return calls.filter((call) => call.method === method && call.path === path)
    .length;
}

/**
 * Stamps the mounted page: a `data-` attribute on the transcript's scroller and
 * an own property on the composer's text area — both nodes `SessionView`'s
 * render created — and one on `window`, which only a navigation clears.
 *
 * React keeps a host node across re-renders and creates a new one on a
 * remount, so the marks live exactly as long as this mount does.
 */
async function markMount(page: Page): Promise<void> {
  await page.evaluate((scrollTestId: string) => {
    const scroller = document.querySelector<HTMLElement>(
      `[data-testid="${scrollTestId}"]`,
    );
    if (scroller === null) throw new Error("markMount: no transcript scroller");
    scroller.dataset.marsMark = "mounted";
    const area = document.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message"]',
    );
    if (area === null) throw new Error("markMount: no composer text area");
    (area as unknown as Record<string, unknown>).marsMark = "mounted";
    (window as unknown as Record<string, unknown>).marsMark = "loaded";
  }, TRANSCRIPT_SCROLL);
}

interface Marks {
  window: unknown;
  scroller: unknown;
  textArea: unknown;
}

/** What [`markMount`] left behind; `null` wherever the node was replaced. */
function readMarks(page: Page): Promise<Marks> {
  return page.evaluate((scrollTestId: string) => {
    const scroller = document.querySelector<HTMLElement>(
      `[data-testid="${scrollTestId}"]`,
    );
    const area = document.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message"]',
    );
    return {
      window: (window as unknown as Record<string, unknown>).marsMark ?? null,
      scroller: scroller?.dataset.marsMark ?? null,
      textArea:
        (area as unknown as Record<string, unknown> | null)?.marsMark ?? null,
    };
  }, TRANSCRIPT_SCROLL);
}

const MOUNTED: Marks = {
  window: "loaded",
  scroller: "mounted",
  textArea: "mounted",
};

function token(page: Page): Promise<string | null> {
  return page.evaluate((key) => window.localStorage.getItem(key), TOKEN_KEY);
}

/** Opens the Terminal panel and waits for the login shell's first prompt. */
async function openTerminal(page: Page): Promise<void> {
  await expect(page.locator(".xterm-rows")).toContainText("@", {
    timeout: 60_000,
  });
}

test("a reconnect keeps the session page mounted, terminal and all", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  // Before the first navigation: both closes work from inside the page, and
  // the init script has to be in place when that page loads.
  await armSocketDrop(context);

  const session = await sessions.launch(api, project.id, {
    message: "hello stub",
  });
  await waitForSessionState(api, session.id, "running", 120_000);

  await page.goto(`/sessions/${session.id}`);
  await expect(connection(page, "live")).toBeVisible({ timeout: 60_000 });

  // The terminal is attached before the first close, so the panel has a live
  // attachment to lose rather than an empty tab.
  await terminalTab(page).click();
  await openTerminal(page);

  // The state a remount would silently throw away.
  await messageBox(page).fill(DRAFT);
  await markMount(page);

  const calls = recordCalls(page);
  const tokenBefore = await token(page);
  expect(tokenBefore).not.toBeNull();

  // --- 1. an outage ----------------------------------------------------------

  expect(await dropConnection(page, 3000)).toBe(1);
  await expect(connection(page, "reconnecting")).toBeVisible({
    timeout: 15_000,
  });
  await expect(connection(page, "live")).toBeVisible({ timeout: 60_000 });

  expect(await readMarks(page)).toEqual(MOUNTED);
  await expect(messageBox(page)).toHaveValue(DRAFT);

  // The side panel kept the operator's tab, and the terminal is detached with
  // the documented escape hatch instead of a fresh shell or a dead one
  // (`SPEC.md`, "WebSocket: session stream": the PTY goes with the connection).
  await expect(terminalTab(page)).toHaveAttribute("aria-selected", "true");
  await expect(
    page.getByText("Terminal disconnected", { exact: true }),
  ).toBeVisible();
  const reconnectTerminal = page.getByRole("button", { name: "Reconnect" });
  await expect(reconnectTerminal).toBeVisible();

  // Nothing re-bootstrapped: the current user was never reloaded and the
  // session was never re-read, which is what a remounted route would do.
  expect(countOf(calls, "GET", "/api/users/me")).toBe(0);
  expect(countOf(calls, "GET", `/api/sessions/${session.id}`)).toBe(0);

  // --- 2. a close with the network up ---------------------------------------

  // A second attachment, so this half starts where the first one did.
  await reconnectTerminal.click();
  await openTerminal(page);

  expect(await closeSockets(page)).toBe(1);

  // It comes straight back — and with the credentials it already had. A close
  // carries no authentication refusal, so nothing is rotated for it (ntepg;
  // `SPEC.md`, "Authentication"): spending the single-use refresh cookie on
  // every close is what turned a stream that could not reopen into a rotation
  // storm that told the reader nothing.
  await expect(connection(page, "live")).toBeVisible({ timeout: 60_000 });
  expect(countOf(calls, "POST", "/api/auth/refresh")).toBe(0);
  expect(await token(page)).toBe(tokenBefore);

  // The reconnect changed nothing about the page: same nodes, same draft, same
  // panel, same `Terminal disconnected`, and still no startup.
  expect(await readMarks(page)).toEqual(MOUNTED);
  await expect(messageBox(page)).toHaveValue(DRAFT);
  await expect(terminalTab(page)).toHaveAttribute("aria-selected", "true");
  await expect(
    page.getByText("Terminal disconnected", { exact: true }),
  ).toBeVisible();
  expect(countOf(calls, "GET", "/api/users/me")).toBe(0);
  expect(countOf(calls, "GET", `/api/sessions/${session.id}`)).toBe(0);

  // And the reopened socket is a working one: the composer can still send.
  await messageBox(page).fill("after the reconnect");
  await page.getByRole("button", { name: /^(Send|Interject)$/ }).click();
  await expect(messageBox(page)).toHaveValue("");
});
