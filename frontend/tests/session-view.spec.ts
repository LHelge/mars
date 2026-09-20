// The session *view* and its streams: what a reader sees when they arrive
// late, reload, watch from a second browser, lose the network, open a terminal
// into the container, copy the link or run an ephemeral profile once.
//
// The lifecycle itself — launch, stop, resume, end, retry, the idle reaper —
// belongs to `sessions.spec.ts`; nothing here asserts a transition for its own
// sake. What is asserted here is the durability sentence of `SPEC.md`,
// "User-facing features" → "Sessions": a session runs when nobody watches it,
// and anyone opening it later sees everything that happened.
//
// The numbers are the recorded fixture's own (`images/stub/fixtures/
// README.md`): three turns whose `result` lines carry the cumulative
// `total_cost_usd` 0.0727456, 0.1447891 and 0.1633155, rendered `$0.0727`,
// `$0.1448` and `$0.1633` on the transcript's result rows. The header's own
// `cost`, `in`, `out` and `cli` refresh only on a state change (`SPEC.md`,
// "WebSocket: session stream"), so a live number is read from the transcript
// and the header is read where the session has just changed state.
//
// The transcript is virtualised and pins itself to the tail, so every read of
// an older row goes through `reveal`, and every assertion on the newest rows
// goes through `pinToLatest`. Both are local copies on purpose: `sessions.spec
// .ts` carries its own and the two files are edited independently.

import { expect, test } from "@playwright/test";
import type {
  APIRequestContext,
  BrowserContext,
  Locator,
  Page,
} from "@playwright/test";

import type { Api, TestUser } from "./utils/test-helpers";
import type { Profile, Project, Session } from "../src/types";
import {
  api,
  armSocketDrop,
  baseUrl,
  createBareRepo,
  createProject,
  createTestUser,
  defaultProfile,
  dropConnection,
  endSession,
  loginViaToken,
  newLoggedInPage,
  waitFor,
  waitForContainerRemoved,
  waitForSessionState,
} from "./utils/test-helpers";

// A container start, a git clone and up to three replayed turns per scenario.
test.setTimeout(180_000);

/** The fixture's three cumulative `total_cost_usd` values, in order. */
const TURN_COST = [0.0727456, 0.1447891, 0.1633155] as const;

/** The same three as `formatUsd(value, 4)` renders them. */
const TURN_COST_TEXT = ["$0.0727", "$0.1448", "$0.1633"] as const;

/**
 * The history page `useSessionSocket` opens with, and the cap the endpoint
 * enforces on `limit` (`SPEC.md`, "Sessions": `GET /sessions/{id}/events`).
 */
const PAGE_SIZE = 200;

/** Every session a scenario launched, ended in `afterEach`. */
let launched: { client: Api; id: string }[] = [];

test.beforeEach(() => {
  launched = [];
});

test.afterEach(async () => {
  for (const session of launched) {
    // Best effort: a scenario that already ended its session is fine, and one
    // that failed must still not leave a container behind.
    await endSession(session.client, session.id).catch(() => undefined);
  }
});

interface Stage {
  user: TestUser;
  client: Api;
  project: Project;
}

/**
 * A fresh user, a local bare upstream and a project that finished cloning, with
 * the browser context already signed in as that user.
 */
async function stage(
  request: APIRequestContext,
  context: BrowserContext,
  prefix: string,
): Promise<Stage> {
  const user = await createTestUser(request, { prefix });
  const client = api(request, user.access_token);
  const repo = createBareRepo("session-view", {
    files: { "src/app.py": 'def main():\n    print("hello")\n' },
  });
  const project = await createProject(client, { remote_url: repo.url });
  await loginViaToken(context, user);
  return { user, client, project };
}

// --- the view's furniture ----------------------------------------------------

/**
 * The session header band: the state badge, the metadata and the actions.
 *
 * Scoped inside `main`, because the application shell's own banner is a
 * `<header>` too and comes first in the document.
 */
function header(page: Page): Locator {
  return page.locator("main header").first();
}

/** One `dd` of the header's metadata list, by its `dt` label. */
function headerField(page: Page, label: string): Locator {
  return header(page)
    .locator("dl > div")
    .filter({ has: page.locator("dt", { hasText: new RegExp(`^${label}$`) }) })
    .locator("dd");
}

/** The virtualised transcript; only the rows in view are in the DOM. */
function transcript(page: Page): Locator {
  return page.getByTestId("transcript-scroll");
}

/** The composer form, which carries no accessible name of its own. */
function composer(page: Page): Locator {
  return page.locator("form").filter({
    has: page.getByLabel("Message", { exact: true }),
  });
}

/** Asserts the header's state badge, which is the pill's own lowercase text. */
async function expectState(page: Page, state: string): Promise<void> {
  await expect(header(page).getByText(state, { exact: true })).toBeVisible({
    timeout: 90_000,
  });
}

/** The connection dot's accessible name (`SessionHeader`, `role="status"`). */
function connection(page: Page, status: string): Locator {
  return header(page).getByRole("status", { name: `Connection ${status}` });
}

/**
 * Pins the transcript back to the newest row after a read-back.
 *
 * Scrolling up unpins the list, and the auto-follow also gives up when a very
 * tall row (the fixture's 8 KiB tool result) is measured after the estimate it
 * was rendered at — so the view can be left showing "Jump to latest" although
 * the reader never scrolled (g4s53). Pressing that control is what a reader
 * does, and what every assertion on the newest rows here needs.
 */
async function pinToLatest(page: Page): Promise<void> {
  const jump = page.getByRole("button", { name: /^Jump to latest/ });
  if ((await jump.count()) > 0) {
    await jump.first().click();
  }
  await transcript(page).evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
}

/**
 * Scrolls the transcript back until `target` is in the DOM and returns it.
 *
 * The transcript is virtualised and pinned to the newest row, so a row from
 * earlier in a turn is not merely off-screen, it is not rendered at all.
 */
async function reveal(page: Page, target: Locator): Promise<Locator> {
  const scroller = transcript(page);
  // From the tail downwards, so a row below the current position is found too.
  await pinToLatest(page);
  for (let step = 0; step < 60; step += 1) {
    if ((await target.count()) > 0) return target;
    const atTop = await scroller.evaluate((element) => {
      const next = Math.max(0, element.scrollTop - element.clientHeight * 0.7);
      const was = element.scrollTop;
      element.scrollTop = next;
      return was === 0;
    });
    if (atTop) break;
    await page.waitForTimeout(80);
  }
  await expect(target.first()).toBeVisible();
  return target;
}

/**
 * How many rows the transcript is folding, which is the identity check every
 * "no duplicates, no gaps" assertion here rests on.
 *
 * The list is virtualised, so counting mounted rows would count a window, not
 * a transcript. What the virtualizer does publish is each mounted row's
 * position in the whole list (`data-index`), and the newest row is mounted
 * whenever the view is pinned to the tail — so the highest index in the DOM,
 * read while pinned, is the length of the store's `order`.
 */
async function rowCount(page: Page): Promise<number> {
  await pinToLatest(page);
  const read = (): Promise<number> =>
    transcript(page).evaluate((element) => {
      let highest = -1;
      for (const row of element.querySelectorAll<HTMLElement>("[data-index]")) {
        const index = Number(row.dataset.index);
        if (Number.isFinite(index) && index > highest) highest = index;
      }
      return highest + 1;
    });
  // Measuring a row can mount the next one, so the count is taken once it has
  // stopped moving rather than on the first render after the scroll.
  let previous = -1;
  for (let step = 0; step < 20; step += 1) {
    const current = await read();
    if (current === previous && current > 0) return current;
    previous = current;
    await page.waitForTimeout(150);
  }
  return previous;
}

// --- session arrangement -----------------------------------------------------

/** Waits until the session's accumulated `cost_usd` is exactly `usd`. */
function waitForCost(
  client: Api,
  sessionId: string,
  usd: number,
): Promise<Session> {
  return waitFor(
    async () => {
      const session = await client.get<Session>(`/sessions/${sessionId}`);
      return Math.abs(session.cost_usd - usd) < 1e-9 ? session : null;
    },
    {
      timeoutMs: 120_000,
      intervalMs: 250,
      description: `session ${sessionId} to have cost ${String(usd)}`,
    },
  );
}

/** Waits for the orchestrator's own accounting to reach turn `index`. */
function waitForTurn(
  client: Api,
  sessionId: string,
  index: number,
): Promise<Session> {
  return waitForCost(client, sessionId, TURN_COST[index]);
}

/**
 * Launches a session from the project's sessions tab, the way a user does, and
 * returns the id the app navigated to.
 */
async function launchFromUi(
  page: Page,
  client: Api,
  project: Project,
  message: string,
  profile?: Profile,
): Promise<string> {
  const chosen = profile ?? (await defaultProfile(client, project.id));

  await page.goto(`/projects/${project.id}?tab=sessions`);
  const form = page.getByRole("form", { name: "Launch a session" });
  await expect(form).toBeVisible();

  await form.getByLabel("Agent profile").selectOption(chosen.id);
  await form.getByLabel("First message (optional)").fill(message);
  await form.getByRole("button", { name: "Launch session" }).click();

  await page.waitForURL(/\/sessions\/[0-9a-f-]{8}-/);
  const id = page.url().slice(page.url().lastIndexOf("/") + 1);
  launched.push({ client, id });
  return id;
}

/** Types `text` into the composer, sends it, and puts the view back on the tail. */
async function compose(page: Page, text: string): Promise<void> {
  const form = composer(page);
  await form.getByLabel("Message", { exact: true }).fill(text);
  await form.getByRole("button", { name: /^(Send|Interject)$/ }).click();
  await pinToLatest(page);
}

/** A running session whose transcript holds the fixture's first two turns. */
async function twoTurns(
  page: Page,
  client: Api,
  project: Project,
): Promise<string> {
  const sessionId = await launchFromUi(page, client, project, "hello stub");
  await waitForTurn(client, sessionId, 0);
  await compose(page, "next");
  await waitForTurn(client, sessionId, 1);
  // The second turn's own result row: the transcript has caught up with the
  // accounting, so the row count below is of a settled transcript.
  await reveal(page, transcript(page).getByText(TURN_COST_TEXT[1]));
  return sessionId;
}

// --- history -----------------------------------------------------------------

test("full history after reload and in a second tab", async ({
  page,
  context,
  request,
  browser,
}) => {
  const { user, client, project } = await stage(request, context, "history");
  const sessionId = await twoTurns(page, client, project);

  const rows = transcript(page);
  const before = await rowCount(page);
  // Two turns of fixture fold into a transcript worth reading back.
  expect(before).toBeGreaterThan(10);

  // Nothing about the session is in this tab: a reload rebuilds it from
  // `GET /sessions/{id}/events` and the socket's replay.
  await page.reload();
  await expectState(page, "running");
  expect(await rowCount(page)).toBe(before);

  // The same rows, each exactly once. A replayed event folded twice would
  // double the row it belongs to, and its neighbour would be mounted with it.
  for (const text of [TURN_COST_TEXT[0], TURN_COST_TEXT[1]]) {
    await expect((await reveal(page, rows.getByText(text))).first()).toBeVisible();
    await expect(rows.getByText(text)).toHaveCount(1);
  }
  await reveal(page, rows.getByText("hello stub", { exact: true }));
  await expect(rows.getByText("hello stub", { exact: true })).toHaveCount(1);
  await reveal(page, rows.getByText("next", { exact: true }));
  await expect(rows.getByText("next", { exact: true })).toHaveCount(1);

  // Another user, another browser: sessions are not private in v1 (`SPEC.md`,
  // "Non-goals"), and the whole history is there for whoever opens it.
  const watcher = await createTestUser(request, { prefix: "watcher" });
  expect(watcher.id).not.toBe(user.id);
  const second = await newLoggedInPage(browser, watcher);
  await second.goto(`/sessions/${sessionId}`);
  await expectState(second, "running");
  expect(await rowCount(second)).toBe(before);
  await reveal(second, transcript(second).getByText(TURN_COST_TEXT[1]));

  // …and it keeps up with what the first browser does next.
  await compose(page, "more");
  await waitForTurn(client, sessionId, 2);
  const denial = transcript(second).getByText(
    /The command was denied by your permission settings/,
  );
  await expect(async () => {
    await reveal(second, denial);
  }).toPass({ timeout: 90_000 });
  await expect(denial).toHaveCount(1);

  await second.context().close();
});

test("reconnect resumes without duplicates", async ({
  page,
  context,
  request,
}) => {
  const { client, project } = await stage(request, context, "reconnect");
  // Before the first navigation: the drop works by closing the socket from
  // inside the page (`armSocketDrop`).
  await armSocketDrop(context);
  const sessionId = await twoTurns(page, client, project);

  const rows = transcript(page);
  const before = await rowCount(page);
  await expect(connection(page, "live")).toBeVisible();

  // Only this browser's network is cut; the orchestrator, the container and
  // the session keep running, which is the whole point.
  expect(await dropConnection(page, 3000)).toBe(1);
  await expect(connection(page, "reconnecting")).toBeVisible({
    timeout: 15_000,
  });
  await expect(connection(page, "live")).toBeVisible({ timeout: 60_000 });

  // The socket reopens at `?after=<lastSeq>`, so the replay starts past what
  // the store already holds: nothing is folded twice.
  expect(await rowCount(page)).toBe(before);
  await reveal(page, rows.getByText(TURN_COST_TEXT[1]));
  await expect(rows.getByText(TURN_COST_TEXT[1])).toHaveCount(1);
  await reveal(page, rows.getByText("hello stub", { exact: true }));
  await expect(rows.getByText("hello stub", { exact: true })).toHaveCount(1);

  // And the reopened socket carries the next turn as usual.
  await compose(page, "more");
  await waitForTurn(client, sessionId, 2);
  await reveal(
    page,
    rows.getByText(/The command was denied by your permission settings/),
  );
  await expect(
    rows.getByText(/The command was denied by your permission settings/),
  ).toHaveCount(1);
  await reveal(page, rows.getByText(TURN_COST_TEXT[2]));
  await expect(rows.getByText(TURN_COST_TEXT[2])).toHaveCount(1);
});

test("older history loads on scroll-up", async ({ page, context, request }) => {
  const { client, project } = await stage(request, context, "paginate");
  const sessionId = await launchFromUi(page, client, project, "hello stub");
  const first = await waitForTurn(client, sessionId, 0);

  // The three recorded turns are 90 events; past them every stdin line is one
  // `Stub reply to:` turn of three. The initial window is `PAGE_SIZE` events,
  // so the session needs more than that before a reload can leave any behind.
  const echoes = Math.ceil((PAGE_SIZE + 30 - first.last_seq) / 3) + 2;
  const texts = Array.from(
    { length: echoes },
    (_unused, index) => `echo ${String(index)}`,
  );
  // Sent straight at the API: these turns are the transcript's ballast, not
  // the thing under test, and the CLI queues stdin lines in order.
  for (const text of texts) {
    await client.send("POST", `/sessions/${sessionId}/input`, {
      kind: "message",
      text,
    });
  }
  const filled = await waitFor(
    async () => {
      const session = await client.get<Session>(`/sessions/${sessionId}`);
      return session.last_seq > PAGE_SIZE + 10 ? session : null;
    },
    {
      timeoutMs: 150_000,
      intervalMs: 500,
      description: `session ${sessionId} to pass ${String(PAGE_SIZE)} events`,
    },
  );
  expect(filled.last_seq).toBeGreaterThan(PAGE_SIZE);

  await page.reload();
  await expectState(page, "running");
  const rows = transcript(page);
  await pinToLatest(page);

  // The newest page only: the first turn is not in the store at all, and the
  // transcript says so above itself.
  await expect(page.getByText("Loading earlier messages")).toBeVisible();
  const windowed = await rowCount(page);
  await expect(rows.getByText("hello stub", { exact: true })).toHaveCount(0);

  // Reading back to the top pages the rest in, once each. A landed page keeps
  // the reader where they were, which puts the scroller back below the
  // trigger — so the top is asked for again until there is nothing left.
  await expect(async () => {
    await transcript(page).evaluate((element) => {
      element.scrollTop = 0;
    });
    await page.waitForTimeout(500);
    await expect(page.getByText("Loading earlier messages")).toHaveCount(0);
  }).toPass({ timeout: 60_000 });

  await reveal(page, rows.getByText("hello stub", { exact: true }));
  await expect(rows.getByText("hello stub", { exact: true })).toHaveCount(1);
  await reveal(page, rows.getByText(TURN_COST_TEXT[0]));
  await expect(rows.getByText(TURN_COST_TEXT[0])).toHaveCount(1);
  expect(await rowCount(page)).toBeGreaterThan(windowed);
});

// --- the terminal ------------------------------------------------------------

test("terminal into a running container", async ({
  page,
  context,
  request,
}) => {
  const { client, project } = await stage(request, context, "terminal");
  const sessionId = await launchFromUi(page, client, project, "hello stub");
  await expectState(page, "running");

  await page.getByRole("tab", { name: "Terminal" }).click();
  // The xterm chunk is fetched on the first open of this tab, and the PTY is
  // opened once the font metrics have settled (`TerminalView`).
  const input = page.locator(".xterm-helper-textarea");
  await expect(input).toBeVisible({ timeout: 30_000 });
  const screen = page.locator(".xterm-rows");
  // The login shell's first prompt says the exec has attached.
  await expect(screen).toContainText("@", { timeout: 30_000 });

  await input.focus();
  await page.keyboard.type("pwd; id -u; echo $MARS_SESSION_ID\n");

  // `/bin/bash -l` as `agent` in the container's own working directory, with
  // the container's environment (`ARCHITECTURE.md`, "Session container
  // specification"; `SPEC.md`, "WebSocket: session stream").
  await expect(screen).toContainText("/session/work", { timeout: 30_000 });
  await expect(screen).toContainText("1000");
  await expect(screen).toContainText(sessionId);

  await page.keyboard.type("exit\n");
  // `terminal_closed` carries the shell's exit code, and the panel reports it.
  await expect(screen).toContainText("[process exited with code 0]", {
    timeout: 30_000,
  });
  await expect(
    page.getByText("Process exited with code 0", { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Reconnect" })).toBeVisible();

  // A `parked` session has no container to exec into, and the server refuses
  // `terminal_open` for one — so the panel does not offer it.
  await composer(page).getByRole("button", { name: "Stop" }).click();
  await waitForSessionState(client, sessionId, "parked", 90_000);
  await waitForContainerRemoved(client, sessionId);
  await expectState(page, "parked");
  await expect(
    page.getByText("Terminal is available while the session is running"),
  ).toBeVisible({ timeout: 30_000 });
  await expect(page.locator(".xterm-helper-textarea")).toHaveCount(0);
});

// --- the link ----------------------------------------------------------------

test("copy link", async ({ page, context, request, browser }) => {
  // Chromium refuses a clipboard write that was not granted (`SPEC.md`,
  // "Frontend", "Copy links").
  await context.grantPermissions(["clipboard-read", "clipboard-write"], {
    origin: baseUrl(),
  });
  const { user, client, project } = await stage(request, context, "copylink");
  const sessionId = await launchFromUi(page, client, project, "hello stub");
  await expectState(page, "running");

  // The canonical route on the current origin, with no query and no token.
  const expected = `${new URL(baseUrl()).origin}/sessions/${sessionId}`;
  await header(page).getByRole("button", { name: "Copy link" }).click();
  await expect(
    header(page).getByRole("button", { name: "Link copied" }),
  ).toBeVisible();
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  expect(copied).toBe(expected);

  // That link, handed to a browser that is not signed in, goes through the
  // login form and lands on the session (`ProtectedRoute`'s `from` state).
  const fresh = await browser.newContext({ baseURL: baseUrl() });
  const cold = await fresh.newPage();
  await cold.goto(expected);
  await cold.waitForURL(/\/login$/);
  await cold.getByLabel("Username").fill(user.username);
  await cold.getByLabel("Password").fill(user.password);
  await cold.getByRole("button", { name: "Sign in" }).click();
  await cold.waitForURL(`**/sessions/${sessionId}`);
  await expectState(cold, "running");
  await fresh.close();
});

// --- an ephemeral run --------------------------------------------------------

test("ephemeral run once from the project page", async ({
  page,
  context,
  request,
}) => {
  const { client, project } = await stage(request, context, "oneshot");

  // A profile written the way an operator writes one: the profiles tab, the
  // editor, `kind: ephemeral` — which turns partial messages off by itself
  // (`orchestrator/src/models/agent_profile.rs`, `default_partial_messages`).
  await page.goto(`/projects/${project.id}?tab=profiles`);
  await page.getByRole("button", { name: "New profile" }).first().click();
  const editor = page.getByRole("form", { name: "New profile" });
  // The label carries the required marker, so it is matched from the front.
  await editor.getByLabel(/^Name/).fill("oneshot");
  await editor.getByLabel("Kind").selectOption("ephemeral");
  await expect(editor.getByLabel("Stream partial messages")).not.toBeChecked();
  await editor.getByRole("button", { name: "Create profile" }).click();

  const profiles = await client.get<Profile[]>(
    `/projects/${project.id}/profiles`,
  );
  const oneshot = profiles.find((profile) => profile.name === "oneshot");
  expect(oneshot).toBeDefined();
  if (oneshot === undefined) return;
  expect(oneshot.kind).toBe("ephemeral");
  expect(oneshot.partial_messages).toBe(false);

  // The sessions tab reshapes itself around the chosen profile: one prompt,
  // required (`SPEC.md`, "User-facing features" → "Agent profiles").
  await page.goto(`/projects/${project.id}?tab=sessions`);
  const launch = page.getByRole("form", { name: "Launch a session" });
  await launch.getByLabel("Agent profile").selectOption(oneshot.id);
  const run = page.getByRole("form", { name: "Run with a message" });
  await expect(run.getByRole("button", { name: "Run" })).toBeDisabled();
  await expect(run.getByText("Give this run a message or a task.")).toBeVisible();

  // The form never sends the request the server would answer 400, so the
  // refusal itself is asserted where it lives (`SPEC.md`, "Sessions").
  const refused = await client.send(
    "POST",
    `/projects/${project.id}/sessions`,
    { profile_id: oneshot.id, base_ref: "main" },
    { allow: [400] },
  );
  expect(refused.status).toBe(400);

  // `Message *` while the run has no task to stand in for it.
  await run.getByLabel(/^Message/).fill("summarise");
  await run.getByRole("button", { name: "Run" }).click();
  await page.waitForURL(/\/sessions\/[0-9a-f-]{8}-/);
  const sessionId = page.url().slice(page.url().lastIndexOf("/") + 1);
  launched.push({ client, id: sessionId });

  // `-p`: the CLI replays every turn and exits, and the session ends itself.
  const done = await waitForSessionState(client, sessionId, "done", 120_000);
  expect(done.ended_at).not.toBeNull();
  await expectState(page, "done");

  // The prompt travelled as argv, not as an input, so there is no
  // `user_message` row for it — it is the session's title instead.
  await expect(
    header(page).getByRole("button", { name: "Edit title" }),
  ).toHaveText("summarise");

  const rows = transcript(page);
  await reveal(
    page,
    rows.getByText(/a tiny demo project used to verify the Mars claude/),
  );
  // An ephemeral run ends on its first `result` (`ARCHITECTURE.md`, "Session
  // lifecycle"), so the fixture's first turn is the whole transcript.
  await reveal(page, rows.getByText(TURN_COST_TEXT[0]));
  await expect(rows.getByText("success").first()).toBeVisible();
  // `done` is a state change, so the header's own row is refreshed with it.
  await expect(headerField(page, "cost")).toHaveText(TURN_COST_TEXT[0]);
  expect(done.cost_usd).toBeCloseTo(TURN_COST[0], 9);

  // No composer at all — not a disabled one (`SPEC.md`, "Frontend",
  // "Composer").
  await expect(composer(page)).toHaveCount(0);
  await expect(page.getByLabel("Message", { exact: true })).toHaveCount(0);

  // An ephemeral session takes no input, ever.
  const rejected = await client.send(
    "POST",
    `/sessions/${sessionId}/input`,
    { kind: "message", text: "one more thing" },
    { allow: [409] },
  );
  expect(rejected.status).toBe(409);
});

// --- the metadata strip ------------------------------------------------------

test("metadata header", async ({ page, context, request }) => {
  const { client, project } = await stage(request, context, "metadata");
  const sessionId = await launchFromUi(page, client, project, "hello stub");
  await expectState(page, "running");

  // What the header can show before a state change: the identity of the
  // session and where it is running.
  await expect(headerField(page, "branch")).toHaveText(`session/${sessionId}`);
  await expect(headerField(page, "base")).toHaveText("main");
  await expect(headerField(page, "container")).not.toHaveText("—");
  await expect(header(page).getByText("conversational")).toBeVisible();
  await expect(
    header(page).getByRole("button", { name: "Edit title" }),
  ).toHaveText("hello stub");

  // `cli` is null until the CLI's first `init`, and the header's row is the
  // one the last state change carried — so the session is stopped, and the
  // `running → parked` change is what brings the recorded id into the strip
  // (`SPEC.md`, "WebSocket: session stream").
  const turn = await waitForTurn(client, sessionId, 0);
  expect(turn.cli_session_id).not.toBeNull();
  await composer(page).getByRole("button", { name: "Stop" }).click();
  await expectState(page, "parked");
  await expect(headerField(page, "cli")).toHaveText(
    String(turn.cli_session_id).slice(0, 12),
  );
  await expect(headerField(page, "cost")).toHaveText(TURN_COST_TEXT[0]);
  await expect(headerField(page, "in")).toHaveText(/^[1-9][\d,]*$/);
  await expect(headerField(page, "out")).toHaveText(/^[1-9][\d,]*$/);

  // The session is the launching user's, which the API carries even though
  // the header's strip does not name them.
  const session = await client.get<Session>(`/sessions/${sessionId}`);
  expect(session.created_by).not.toBeNull();
});
