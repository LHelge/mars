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
// goes through `pinToLatest`, both from `tests/utils/transcript.ts`.

import type { Locator, Page } from "@playwright/test";

import type { Api } from "./utils/test-helpers";
import type { Profile, Project, Session } from "../src/types";
import { expect, test } from "./utils/fixtures";
import type { SessionTracker } from "./utils/fixtures";
import {
  armSocketDrop,
  baseUrl,
  createTestUser,
  defaultProfile,
  dropConnection,
  loginViaToken,
  newLoggedInPage,
  pinToLatest,
  reveal,
  rowCount,
  transcript,
  waitFor,
  waitForContainerRemoved,
  waitForSessionState,
} from "./utils/test-helpers";

// The upstream every scenario here clones: the file the fixture's `Edit` tool
// rewrites.
test.use({ repoFiles: { "src/app.py": 'def main():\n    print("hello")\n' } });

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

/**
 * Sends one ballast turn, tolerating the owner's back-pressure.
 *
 * A burst of inputs can outrun the session owner's command channel; the
 * orchestrator refuses the one that does not fit with 409 `input queue full`
 * rather than reordering it behind the next one that does
 * (`orchestrator/src/session/registry.rs`). The documented answer is to resend,
 * which is what a composer's user would do, so the send is retried until the
 * owner has drained enough to take it.
 */
async function sendBallast(
  client: Api,
  sessionId: string,
  text: string,
): Promise<void> {
  await waitFor(
    async () => {
      const result = await client.send(
        "POST",
        `/sessions/${sessionId}/input`,
        { kind: "message", text },
        { allow: [409] },
      );
      if (result.status === 202) return true;
      if (result.text.includes("input queue full")) return null;
      throw new Error(
        `POST /sessions/${sessionId}/input answered ${String(result.status)}: ${result.text}`,
      );
    },
    {
      timeoutMs: 60_000,
      intervalMs: 250,
      description: `session ${sessionId} to accept one more input`,
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
  sessions: SessionTracker,
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
  return sessions.track(client, id);
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
  sessions: SessionTracker,
  client: Api,
  project: Project,
): Promise<string> {
  const sessionId = await launchFromUi(
    page,
    sessions,
    client,
    project,
    "hello stub",
  );
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
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const sessionId = await twoTurns(page, sessions, api, project);

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
  await waitForTurn(api, sessionId, 2);
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
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  // Before the first navigation: the drop works by closing the socket from
  // inside the page (`armSocketDrop`).
  await armSocketDrop(context);
  const sessionId = await twoTurns(page, sessions, api, project);

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
  await waitForTurn(api, sessionId, 2);
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

test("older history loads on scroll-up", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const sessionId = await launchFromUi(
    page,
    sessions,
    api,
    project,
    "hello stub",
  );
  const first = await waitForTurn(api, sessionId, 0);

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
    await sendBallast(api, sessionId, text);
  }
  const filled = await waitFor(
    async () => {
      const session = await api.get<Session>(`/sessions/${sessionId}`);
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
  // A landed page keeps the reader where they were, which puts the scroller
  // back below the trigger, so the top is asked for again until nothing is
  // left: each pass scrolls to the top and then waits for the indicator to go,
  // rather than sleeping for a page fetch that is usually far quicker.
  await expect(async () => {
    await transcript(page).evaluate((element) => {
      element.scrollTop = 0;
    });
    await expect(page.getByText("Loading earlier messages")).toHaveCount(0, {
      timeout: 2000,
    });
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
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const sessionId = await launchFromUi(
    page,
    sessions,
    api,
    project,
    "hello stub",
  );
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
  await waitForSessionState(api, sessionId, "parked", 90_000);
  await waitForContainerRemoved(api, sessionId);
  await expectState(page, "parked");
  await expect(
    page.getByText("Terminal is available while the session is running"),
  ).toBeVisible({ timeout: 30_000 });
  await expect(page.locator(".xterm-helper-textarea")).toHaveCount(0);
});

// --- the link ----------------------------------------------------------------

test("copy link", async ({
  page,
  context,
  browser,
  user,
  api,
  project,
  sessions,
}) => {
  // Chromium refuses a clipboard write that was not granted (`SPEC.md`,
  // "Frontend", "Copy links").
  await context.grantPermissions(["clipboard-read", "clipboard-write"], {
    origin: baseUrl(),
  });
  await loginViaToken(context, user);
  const sessionId = await launchFromUi(
    page,
    sessions,
    api,
    project,
    "hello stub",
  );
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
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);

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

  const profiles = await api.get<Profile[]>(
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
  const refused = await api.send(
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
  const sessionId = sessions.track(
    api,
    page.url().slice(page.url().lastIndexOf("/") + 1),
  );

  // `-p`: the CLI replays every turn and exits, and the session ends itself.
  const done = await waitForSessionState(api, sessionId, "done", 120_000);
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
  const rejected = await api.send(
    "POST",
    `/sessions/${sessionId}/input`,
    { kind: "message", text: "one more thing" },
    { allow: [409] },
  );
  expect(rejected.status).toBe(409);
});

// --- the metadata strip ------------------------------------------------------

test("metadata header", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const sessionId = await launchFromUi(
    page,
    sessions,
    api,
    project,
    "hello stub",
  );
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
  const turn = await waitForTurn(api, sessionId, 0);
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
  const session = await api.get<Session>(`/sessions/${sessionId}`);
  expect(session.created_by).not.toBeNull();
});
