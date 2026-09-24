// The session lifecycle on real containers: launch, transcript replay, a
// message mid-turn, stop, resume, end, a CLI that exits non-zero, retry and the
// idle reaper (`SPEC.md`, "User-facing features" → "Sessions"; "Sessions" table;
// `ARCHITECTURE.md`, "Session lifecycle", "Launch sequence", "Stop semantics",
// "Cost accounting").
//
// Every scenario runs an actual session container over the stub image the
// end-to-end stack builds, so the numbers asserted here are the recorded
// fixture's own (`images/stub/fixtures/README.md`): three turns whose `result`
// lines carry the *cumulative* `total_cost_usd` 0.0727456, 0.1447891 and
// 0.1633155, which the increase-over-previous rule turns into exactly those
// three session totals, rendered `$0.0727`, `$0.1448` and `$0.1633` by
// `formatUsd(value, 4)`.
//
// The stub's knobs are passed the documented way: a project secret declared on
// the default profile, which the launcher resolves into the container's
// environment (`ARCHITECTURE.md`, "Launch sequence"; `images/stub/claude`).
//
// **Where the header's numbers come from.** The socket sends a `session` frame
// on open and "on every state change" (`SPEC.md`, "WebSocket: session stream"),
// and nothing else refreshes the row the header draws. So `cost`, `in`, `out`
// and `cli` in the header are the values as of the last state change, and a
// turn's own cost is asserted where it arrives live — on the transcript's
// result row, which renders the `result` event's `cost_usd` — with the session's
// accumulated counters asserted through the API beside it.

import type { Locator, Page } from "@playwright/test";

import type { Api } from "./utils/test-helpers";
import type { Project, Session } from "../src/types";
import { expect, test } from "./utils/fixtures";
import type { SessionTracker } from "./utils/fixtures";
import {
  commitInSessionWorkClone,
  defaultProfile,
  endSession,
  gitRevParse,
  loginViaToken,
  mirrorPath,
  openRow,
  reveal,
  sendInput,
  sessionContainers,
  setProfileIdleTimeout,
  setProfileSecrets,
  setProjectSecret,
  transcript,
  waitFor,
  waitForContainerRemoved,
  waitForSessionState,
} from "./utils/test-helpers";

// The upstream every scenario here clones: the file the fixture's `Edit` tool
// rewrites.
test.use({ repoFiles: { "src/app.py": 'def main():\n    print("hello")\n' } });

// A container start, a git clone and three replayed turns; the idle scenario
// additionally waits out a cron period.
test.setTimeout(180_000);

/** The fixture's three cumulative `total_cost_usd` values, in order. */
const TURN_COST = [0.0727456, 0.1447891, 0.1633155] as const;

/** The same three as `formatUsd(value, 4)` renders them. */
const TURN_COST_TEXT = ["$0.0727", "$0.1448", "$0.1633"] as const;

/** Declares each knob on the default profile with its value as a project secret. */
async function stubKnobs(
  client: Api,
  projectId: string,
  knobs: Record<string, string>,
): Promise<void> {
  for (const [name, value] of Object.entries(knobs)) {
    await setProjectSecret(client, projectId, name, value);
  }
  await setProfileSecrets(client, projectId, Object.keys(knobs));
}

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

/** Waits until the session has committed an event past `seq`. */
function waitForSeqPast(
  client: Api,
  sessionId: string,
  seq: number,
): Promise<Session> {
  return waitFor(
    async () => {
      const session = await client.get<Session>(`/sessions/${sessionId}`);
      return session.last_seq > seq ? session : null;
    },
    {
      timeoutMs: 60_000,
      intervalMs: 250,
      description: `session ${sessionId} to commit an event past ${String(seq)}`,
    },
  );
}

/**
 * Waits until the session's accumulated cost is the fixture's value after turn
 * `index` — the orchestrator's own cost accounting, which the header shows as
 * of the last state change.
 */
function waitForTurn(
  client: Api,
  sessionId: string,
  index: number,
): Promise<Session> {
  const cost = TURN_COST[index];
  if (cost === undefined) {
    throw new Error(`the transcript fixture has no turn ${String(index)}`);
  }
  return waitForCost(client, sessionId, cost);
}

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
): Promise<string> {
  const profile = await defaultProfile(client, project.id);

  await page.goto(`/projects/${project.id}?tab=sessions`);
  const form = page.getByRole("form", { name: "Launch a session" });
  await expect(form).toBeVisible();

  await form.getByLabel("Agent profile").selectOption(profile.id);
  // The base ref is left at the project's own default, which names `main`.
  await expect(
    form.getByLabel("Base ref").locator("option").first(),
  ).toHaveText(`Project default (${project.default_branch ?? "main"})`);
  await form.getByLabel("First message (optional)").fill(message);
  await form.getByRole("button", { name: "Launch session" }).click();

  await page.waitForURL(/\/sessions\/[0-9a-f-]{8}-/);
  const id = page.url().slice(page.url().lastIndexOf("/") + 1);
  return sessions.track(client, id);
}

/** Types `text` into the composer and sends it. */
async function compose(page: Page, text: string): Promise<void> {
  const form = composer(page);
  await form.getByLabel("Message", { exact: true }).fill(text);
  await form.getByRole("button", { name: /^(Send|Interject)$/ }).click();
}

test("launch with a first message and watch the transcript", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);

  const sessionId = await launchFromUi(page, sessions, api, project, "hello stub");

  // `creating → running` happens on stdin attach, not on `init`
  // (`ARCHITECTURE.md`, "Launch sequence"; ADR 0032).
  await expectState(page, "running");
  await expect(headerField(page, "branch")).toHaveText(`session/${sessionId}`);
  await expect(headerField(page, "container")).not.toHaveText("—");
  await expect(
    header(page).getByRole("button", { name: "Edit title" }),
  ).toHaveText("hello stub");

  const rows = transcript(page);
  // What the orchestrator accumulated, and the `cli_session_id` the first
  // `init` recorded (null until then; `ARCHITECTURE.md`, "Launch sequence").
  const session = await waitForTurn(api, sessionId, 0);
  expect(session.cli_session_id).not.toBeNull();
  expect(session.input_tokens).toBeGreaterThan(0);
  expect(session.output_tokens).toBeGreaterThan(0);

  // The whole first turn arrives under a reader who never touches the
  // scroller, and it carries the fixture's 8 KiB `Read` result: measuring a row
  // that tall moves the bottom far out of reach, and the transcript has to
  // follow it there. Its last row — the turn's own result, carrying the cost —
  // is on screen without anything being pressed, and nothing offers to jump.
  await expect(rows.getByText(TURN_COST_TEXT[0])).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^Jump to latest/ }),
  ).toHaveCount(0);
  await expect(rows.getByText("success").first()).toBeVisible();

  await reveal(
    page,
    rows.getByText(/a tiny demo project used to verify the Mars claude/),
  );

  // Everything above it is reached by reading back, because the transcript is
  // virtualised and pinned to the end.
  const read = await reveal(
    page,
    rows.getByRole("button", { name: "Read /session/work/README.md" }),
  );
  // The `Read` card: a collapsed one-line summary that expands to its result.
  await expect(read).toHaveAttribute("aria-expanded", "false");
  await read.click();
  // The result itself, in the `<pre>` the collapsible renders it in.
  await expect(
    rows.locator("pre").filter({ hasText: "# Greeter" }).first(),
  ).toBeVisible();

  // The second `Read` of the turn returns far more than 40 lines, so its result
  // is collapsed with an expand control (`components/CollapsibleLines.tsx`).
  // The task text expects this in the second turn; the recorded fixture puts
  // the oversized result in the first one.
  const changelog = await reveal(
    page,
    rows.getByRole("button", { name: "Read /session/work/docs/CHANGELOG.md" }),
  );
  await changelog.click();
  const expand = rows.getByRole("button", {
    name: /^Show all \d+ lines of result$/,
  });
  await expect(expand).toBeVisible();
  await expand.click();
  await expect(
    rows.getByRole("button", { name: "Collapse result" }),
  ).toBeVisible();

  // The message the session was launched with, and the `init` the CLI writes
  // once it has read its first stdin line.
  await reveal(page, rows.getByText("Session started (stub)"));
  await reveal(page, rows.getByText("hello stub", { exact: true }));
});

test("second and third turns render subagent, edit diff, shell, deltas and denial", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const sessionId = await launchFromUi(page, sessions, api, project, "hello stub");
  await waitForTurn(api, sessionId, 0);

  await compose(page, "next");
  const rows = transcript(page);

  await waitForTurn(api, sessionId, 1);
  await reveal(page, rows.getByText(TURN_COST_TEXT[1]));

  // Tool rows start folded to a header that says what the call was: the
  // `Bash` commit shows its description there, and its command — monospace,
  // prefixed by the shell renderer — once the row is opened.
  await openRow(
    page,
    rows.getByRole("button", {
      name: "Bash Stage and commit the greeting change",
    }),
  );
  await expect(
    rows.getByText(/git add -A && git commit/).first(),
  ).toBeVisible();

  // The `Edit` row's header carries the path; opened, it renders a diff with
  // both halves of the edit.
  await openRow(
    page,
    rows.getByRole("button", { name: "Edit /session/work/src/app.py" }),
  );
  await expect(rows.getByText('print("hello, world")').first()).toBeVisible();
  await expect(rows.getByText('print("hello")').first()).toBeVisible();

  // The subagent call is a collapsible nested transcript that starts folded,
  // and so does the tool row inside it.
  await openRow(page, rows.getByRole("button", { name: /general-purpose/ }));
  const children = page.getByTestId("subagent-children").first();
  await openRow(
    page,
    children.getByRole("button", {
      name: "Bash Search recursively for literal token 'main'",
    }),
  );
  await expect(
    children.getByText("grep -rn 'main' . --exclude-dir=.git"),
  ).toBeVisible();

  await compose(page, "more");

  // The third turn's assistant text arrives as `text_delta`s folded into one
  // message; the intermediate `streaming-cursor` render is too brief to catch
  // reliably without the delay knob, so the folded text is what is asserted.
  await waitForTurn(api, sessionId, 2);
  await reveal(
    page,
    rows.getByText(/The command was denied by your permission settings/),
  );
  await reveal(page, rows.getByText(/^Permission denied: Bash — /));
  await reveal(page, rows.getByText(TURN_COST_TEXT[2]));
});

test("after the fixture is exhausted the stub echoes", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const sessionId = await launchFromUi(page, sessions, api, project, "hello stub");

  // Three turns of fixture, then every stdin line yields `Stub reply to: …`.
  await waitForTurn(api, sessionId, 0);
  await compose(page, "next");
  await waitForTurn(api, sessionId, 1);
  await compose(page, "more");
  await waitForTurn(api, sessionId, 2);

  const before = (await waitForTurn(api, sessionId, 2)).last_seq;
  await compose(page, "echo me");
  // The echo turn's own `result` reports a lower cumulative cost than the
  // fixture's last one, so it adds nothing to the counters: the sequence is
  // what says the turn landed (`ARCHITECTURE.md`, "Cost accounting").
  await waitForSeqPast(api, sessionId, before + 1);
  await reveal(page, transcript(page).getByText("Stub reply to: echo me"));
});

test("interject mid-turn", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  // 200 ms a line paces the fixture's first turn (58 lines) over about twelve
  // seconds, which is the window the interjection has to be typed and sent in
  // — ample, and roughly half the wall-clock the scenario used to spend
  // replaying both turns at 400 ms (`images/stub/claude`,
  // `MARS_STUB_LINE_DELAY_MS`).
  await stubKnobs(api, project.id, { MARS_STUB_LINE_DELAY_MS: "200" });

  const sessionId = await launchFromUi(
    page,
    sessions,
    api,
    project,
    "hello stub",
  );
  await expectState(page, "running");

  // While the turn streams the composer's button is `Interject`.
  const form = composer(page);
  const button = form.getByRole("button", { name: "Interject" });
  await expect(button).toBeVisible({ timeout: 60_000 });

  await form.getByLabel("Message", { exact: true }).fill("wait for me");
  await button.click();

  const rows = transcript(page);
  // Optimistic first, then confirmed by the `user_message` event: the
  // `client_id` reconciliation must leave exactly one row.
  const interjection = rows.getByText("wait for me", { exact: true });
  await expect(interjection).toBeVisible();
  await expect(interjection).toHaveCount(1);

  // The interjected message is queued by the CLI as the next turn, so the first
  // turn finishes and the second one then plays.
  await waitForTurn(api, sessionId, 0);
  await waitForTurn(api, sessionId, 1);
  await reveal(page, rows.getByRole("button", { name: "Agent" }));
  await reveal(page, rows.getByText("wait for me", { exact: true }));
  await expect(rows.getByText("wait for me", { exact: true })).toHaveCount(1);
});

test("stop parks the session and shows stopped", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  await stubKnobs(api, project.id, { MARS_STUB_LINE_DELAY_MS: "400" });

  const sessionId = await launchFromUi(page, sessions, api, project, "hello stub");
  await expectState(page, "running");
  // Wait for the CLI's `init`, so the stop lands mid-turn and the header has a
  // `cli_session_id` to show once the state change refreshes its row.
  await expect(
    transcript(page).getByText("Session started (stub)"),
  ).toBeVisible({ timeout: 60_000 });

  // Mid-turn, so the `state_change` records `SIGINT` and the transcript says
  // "stopped" rather than "killed" (`ARCHITECTURE.md`, "Stop semantics").
  await composer(page).getByRole("button", { name: "Stop" }).click();

  await expectState(page, "parked");
  await expect(header(page).getByText("waiting")).toBeVisible();
  await reveal(page, transcript(page).getByText("Stopped (SIGINT)"));
  await expect(headerField(page, "cli")).not.toHaveText("—");

  const parked = await waitForContainerRemoved(api, sessionId);
  expect(parked.state).toBe("parked");
  expect(parked.container_id).toBeNull();

  // A parked session accepts input, which is what relaunches it.
  const form = composer(page);
  await expect(form.getByLabel("Message", { exact: true })).toBeEnabled();
  await expect(
    form.getByText("Sending will relaunch the session"),
  ).toBeVisible();
});

test("sending a message to a parked session relaunches it", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const sessionId = await launchFromUi(page, sessions, api, project, "hello stub");
  const running = await waitForTurn(api, sessionId, 0);
  const cliSessionId = running.cli_session_id;
  expect(cliSessionId).not.toBeNull();

  await composer(page).getByRole("button", { name: "Stop" }).click();
  await waitForSessionState(api, sessionId, "parked");
  await expectState(page, "parked");

  await compose(page, "resumed hello");

  await expectState(page, "running");
  const resumed = await waitForSessionState(api, sessionId, "running");
  // The resumed process is `claude --resume <cli_session_id>`, so the id the
  // second `init` repeats is the first one's.
  expect(resumed.cli_session_id).toBe(cliSessionId);
  await reveal(page, transcript(page).getByText("Session resumed (stub)"));

  // The resumed process replays the fixture from its first turn, so the reply
  // is the same text again — `.last()` is the new one — and the cost counters
  // gain that turn's value a second time, because `--resume` does not carry the
  // earlier process's totals forward (`ARCHITECTURE.md`, "Cost accounting").
  await waitForCost(api, sessionId, TURN_COST[0] * 2);
  await reveal(
    page,
    transcript(page)
      .getByText(/a tiny demo project used to verify the Mars/)
      .last(),
  );
});

test("end moves to done and disables the composer", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const sessionId = await launchFromUi(page, sessions, api, project, "hello stub");
  await waitForTurn(api, sessionId, 0);

  const actions = header(page);
  await actions.getByRole("button", { name: "End", exact: true }).click();
  await actions.getByRole("button", { name: "End the session" }).click();

  await expectState(page, "done");
  const done = await waitForSessionState(api, sessionId, "done");
  expect(done.ended_at).not.toBeNull();
  // `done` is a state change, so the header's row is refreshed with it.
  await expect(headerField(page, "cost")).toHaveText(TURN_COST_TEXT[0]);
  await expect(headerField(page, "out")).toHaveText(/^[1-9][\d,]*$/);

  const form = composer(page);
  await expect(form.getByLabel("Message", { exact: true })).toBeDisabled();
  await expect(form.getByText("Session has ended")).toBeVisible();

  // `done` accepts no input at all (`ARCHITECTURE.md`, "Session lifecycle").
  const rejected = await api.send(
    "POST",
    `/sessions/${sessionId}/input`,
    { kind: "message", text: "after the end" },
    { allow: [409] },
  );
  expect(rejected.status).toBe(409);

  // Ending fetches the session branch into the mirror, where it is kept under
  // its own ref (`ARCHITECTURE.md`, "Stop semantics"; "Git model").
  expect(
    gitRevParse(mirrorPath(project.id), `refs/sessions/${sessionId}`),
  ).toMatch(/^[0-9a-f]{40}$/);
});

test("ending a session right after launch leaves no container", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const sessionId = await launchFromUi(page, sessions, api, project, "hello stub");

  // No wait for `running`: the End button is offered while the session is
  // still `creating`, and pressing it there cancels the launch
  // (`SPEC.md`, "Sessions"; `ARCHITECTURE.md`, "Session lifecycle", "A session
  // ended while it is creating").
  const actions = header(page);
  await actions.getByRole("button", { name: "End", exact: true }).click();
  await actions.getByRole("button", { name: "End the session" }).click();

  await expectState(page, "done");
  const done = await waitForSessionState(api, sessionId, "done");
  expect(done.ended_at).not.toBeNull();
  expect(done.container_id).toBeNull();

  const form = composer(page);
  await expect(form.getByLabel("Message", { exact: true })).toBeDisabled();
  await expect(form.getByText("Session has ended")).toBeVisible();

  // The leak this scenario exists for: the refused end used to let the launch
  // carry on and start a container nothing would remove (task `qhyhw`). The
  // engine is the only witness that can say it did not.
  expect(sessionContainers(sessionId)).toEqual([]);
});

test("a CLI that exits non-zero fails the session and retry parks it", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  await stubKnobs(api, project.id, {
    MARS_STUB_EXIT_AFTER_TURNS: "1",
    MARS_STUB_EXIT_CODE: "1",
  });

  const sessionId = await launchFromUi(page, sessions, api, project, "fail please");

  // An unrecoverable CLI error is `failed`, not `parked` (`ARCHITECTURE.md`,
  // "Session lifecycle").
  await expectState(page, "failed");
  const failed = await waitForSessionState(api, sessionId, "failed");
  expect(failed.error).not.toBeNull();
  // The error is in the header's alert and, once more, in the state pill's
  // screen-reader suffix; the alert is the visible one.
  await expect(
    header(page).getByText(String(failed.error)).last(),
  ).toBeVisible();

  // Undeclaring the knobs is what keeps them out of the relaunched container:
  // only a profile's declared secrets are resolved into its environment
  // (`ARCHITECTURE.md`, "Launch sequence").
  await setProfileSecrets(api, project.id, []);

  await header(page).getByRole("button", { name: "Retry" }).click();
  await header(page).getByLabel("Retry message").fill("try again");
  await header(page).getByRole("button", { name: "Relaunch" }).click();

  // `failed → parked` on retry, then `parked → running` on the relaunch.
  await expect(transcript(page).getByText(/^failed → parked: /)).toBeVisible({
    timeout: 60_000,
  });
  await expectState(page, "running");
  // The relaunched process replays the fixture from its first turn, so the same
  // reply arrives a second time and the cost gains that turn's value again.
  await expect(
    transcript(page)
      .getByText(/a tiny demo project used to verify the Mars/)
      .last(),
  ).toBeVisible({ timeout: 90_000 });
  await waitForCost(api, sessionId, TURN_COST[0] * 2);
});

test("title defaults to the message's first line, truncated to 80 characters", async ({
  api,
  project,
  sessions,
}) => {

  const multiline = await sessions.launch(api, project.id, {
    base_ref: "main",
    message: "Line one\nLine two",
  });
  // `POST /projects/{pid}/sessions` answers 201 with `state: creating`
  // (`SPEC.md`, "Sessions").
  expect(multiline.state).toBe("creating");
  expect(multiline.title).toBe("Line one");

  const long = await sessions.launch(api, project.id, {
    base_ref: "main",
    message: "x".repeat(200),
  });
  expect(long.title).toHaveLength(80);

  const untitled = await sessions.launch(api, project.id, {
    base_ref: "main",
  });
  expect(untitled.title).toBeNull();
});

test("an idle session is parked without user action", async ({
  api,
  project,
  sessions,
}) => {
  // One second is the model's floor; the reaper is a cron job on a 60 s period,
  // so the park lands at the next tick (`ARCHITECTURE.md`, "Session owner
  // task", point 4).
  await setProfileIdleTimeout(api, project.id, 1);

  const session = await sessions.launch(api, project.id, {
    base_ref: "main",
    message: "hello stub",
  });
  await waitForSessionState(api, session.id, "running", 90_000);

  const parked = await waitForSessionState(api, session.id, "parked", 90_000);
  expect(parked.parked_at).not.toBeNull();
  // Idleness parks a conversational session; it never fails it.
  expect(parked.error).toBeNull();

  // The park is the reaper's, not a user's, and a message still resumes it.
  await waitForContainerRemoved(api, session.id);
  await sendInput(api, session.id, "back to work");
  await waitForSessionState(api, session.id, "running", 90_000);
});

test("deleting an ended session warns of its unmerged commits and takes its branch", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  await loginViaToken(context, user);
  const session = await sessions.launch(api, project.id, { base_ref: "main" });
  await waitForSessionState(api, session.id, "running", 120_000);

  // What an agent leaves behind: a commit the default branch does not have.
  commitInSessionWorkClone(
    session.id,
    { "NOTES.md": "work\n" },
    "feat: unmerged",
  );
  // Ending fetches the branch back into the mirror (`ARCHITECTURE.md`, "Stop
  // semantics"), which is what gives it an `ahead` to warn about.
  await endSession(api, session.id);
  await waitForSessionState(api, session.id, "done", 60_000);
  const mirror = mirrorPath(project.id);
  expect(gitRevParse(mirror, `refs/sessions/${session.id}`)).toMatch(
    /^[0-9a-f]{40}$/,
  );

  await page.goto(`/projects/${project.id}?tab=sessions`);
  const label = session.title ?? `session ${session.id.slice(0, 8)}`;
  await page.getByRole("button", { name: `Delete ${label}` }).click();

  // `SPEC.md`, "Frontend", Confirmations: a warning, not a refusal.
  await expect(
    page.getByText(
      `${session.branch ?? `refs/sessions/${session.id}`} has 1 commit not on main; they will be lost.`,
    ),
  ).toBeVisible({ timeout: 30_000 });
  // The row's button, and below it the panel's, which confirms.
  await page
    .getByRole("button", { name: `Delete ${label}` })
    .last()
    .click();
  await expect(
    page.getByRole("button", { name: `Delete ${label}` }),
  ).toHaveCount(0, { timeout: 30_000 });

  // The ref went with the session (ADR 0049): gone from the mirror, from the
  // API's list and from the Branches tab.
  expect(() => gitRevParse(mirror, `refs/sessions/${session.id}`)).toThrow();
  const listed = await api.get<{ session_id: string }[]>(
    `/projects/${project.id}/git/session-branches`,
  );
  expect(listed.map((branch) => branch.session_id)).not.toContain(session.id);
  await page.goto(`/projects/${project.id}?tab=branches`);
  await expect(page.getByText("No session branches yet")).toBeVisible({
    timeout: 30_000,
  });
});
