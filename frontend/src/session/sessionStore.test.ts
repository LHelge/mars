// Fixture-driven tests for the transcript fold of `SPEC.md`, "Session state".
// Each `describe` loads one hand-written `AgentEvent[]` fixture, applies it
// event by event and asserts the folded state explicitly (no snapshots).

import { describe, expect, it } from "vitest";

import type { AgentEvent, Session } from "../types";
import {
  createSessionStore,
  optimisticId,
  type AssistantTextMessage,
  type Message,
  type ResultMessage,
  type SessionStore,
  type SystemMessage,
  type ToolMessage,
} from "./sessionStore";

import gitOpsFixture from "./fixtures/git_ops.json";
import rawAndUnknownFixture from "./fixtures/raw_and_unknown.json";
import simpleTurnFixture from "./fixtures/simple_turn.json";
import stopAndParkFixture from "./fixtures/stop_and_park.json";
import subagentFixture from "./fixtures/subagent.json";
import toolUseFixture from "./fixtures/tool_use.json";

// The fixtures are JSON, so TypeScript widens their literal `kind` to `string`.
function events(fixture: unknown): AgentEvent[] {
  return fixture as AgentEvent[];
}

const simpleTurn = events(simpleTurnFixture);
const toolUse = events(toolUseFixture);
const subagent = events(subagentFixture);
const stopAndPark = events(stopAndParkFixture);
const gitOps = events(gitOpsFixture);
const rawAndUnknown = events(rawAndUnknownFixture);

function applyAll(
  store: ReturnType<typeof createSessionStore>,
  list: AgentEvent[],
): SessionStore {
  for (const event of list) store.getState().applyEvent(event);
  return store.getState();
}

function folded(list: AgentEvent[]): SessionStore {
  return applyAll(createSessionStore(), list);
}

function at(state: SessionStore, index: number): Message {
  const id = state.order[index];
  const message = state.messages[id];
  expect(message, `no message at order[${index}]`).toBeDefined();
  return message;
}

function tool(state: SessionStore, id: string): ToolMessage {
  const message = state.messages[id];
  expect(message?.kind).toBe("tool");
  return message as ToolMessage;
}

function fakeSession(): Session {
  return {
    id: "00000000-0000-0000-0000-0000000000aa",
    project_id: "00000000-0000-0000-0000-0000000000bb",
    profile_id: "00000000-0000-0000-0000-0000000000cc",
    kind: "conversational",
    created_by: null,
    title: "Fake session",
    task_id: null,
    handoff_id: null,
    state: "running",
    base_ref: "main",
    branch: "sessions/fake",
    container_id: null,
    cli_session_id: null,
    last_seq: 0,
    last_activity_at: "2026-01-02T13:00:00Z",
    cost_usd: 0,
    input_tokens: 0,
    output_tokens: 0,
    error: null,
    created_at: "2026-01-02T12:59:00Z",
    parked_at: null,
    ended_at: null,
  };
}

describe("simple_turn fixture", () => {
  it("folds init, user message, deltas, text and result", () => {
    const state = folded(simpleTurn);

    expect(state.order).toEqual(["e1", "e2", "e3", "e7"]);
    const init = at(state, 0) as SystemMessage;
    expect(init.kind).toBe("system");
    expect(init.level).toBe("info");
    expect(init.text).toBe("Session started (claude-test-model)");

    const user = at(state, 1);
    expect(user).toMatchObject({ kind: "user", text: "Rename the widget" });

    // The deltas fold into one message whose id survives the completing `text`.
    const assistant = at(state, 2) as AssistantTextMessage;
    expect(assistant.kind).toBe("assistant_text");
    expect(assistant.id).toBe("e3");
    expect(assistant.text).toBe("Renaming the widget.");
    expect(assistant.streaming).toBe(false);

    const result = at(state, 3) as ResultMessage;
    expect(result.kind).toBe("result");
    expect(result).toMatchObject({
      subtype: "success",
      is_error: false,
      num_turns: 1,
      duration_ms: 4200,
      cost_usd: 0.0123,
    });

    expect(state.pendingTools).toEqual({});
    expect(state.subagents).toEqual({});
    expect(state.turnActive).toBe(false);
    expect(state.lastSeq).toBe(7);
    expect(state.oldestSeq).toBe(1);
  });

  it("sets turnActive while the turn runs", () => {
    const store = createSessionStore();
    expect(store.getState().turnActive).toBe(false);
    applyAll(store, simpleTurn.slice(0, 3));
    expect(store.getState().turnActive).toBe(true);
    applyAll(store, simpleTurn.slice(3));
    expect(store.getState().turnActive).toBe(false);
  });

  it("starts a new streaming message when a delta follows a completed text", () => {
    const store = createSessionStore();
    applyAll(store, simpleTurn);
    store.getState().applyEvent({
      seq: 8,
      ts: "2026-01-02T10:00:07Z",
      kind: "text_delta",
      text: "More",
    });
    const state = store.getState();
    expect(state.order).toEqual(["e1", "e2", "e3", "e7", "e8"]);
    const first = state.messages.e3 as AssistantTextMessage;
    expect(first.text).toBe("Renaming the widget.");
    const second = state.messages.e8 as AssistantTextMessage;
    expect(second.text).toBe("More");
    expect(second.streaming).toBe(true);
  });
});

describe("tool_use fixture", () => {
  it("completes both tool messages and empties pendingTools", () => {
    const state = folded(toolUse);

    expect(state.order).toEqual(["e1", "e3"]);
    const edit = tool(state, "e1");
    expect(edit.name).toBe("Edit");
    expect(edit.running).toBe(false);
    expect(edit.is_error).toBe(false);
    expect(edit.truncated).toBe(false);
    expect(edit.result).toBe("The file /repo/src/widget.ts has been updated.");

    const bash = tool(state, "e3");
    expect(bash.name).toBe("Bash");
    expect(bash.running).toBe(false);
    expect(bash.is_error).toBe(true);
    expect(bash.truncated).toBe(true);
    // Structured content is stored as delivered.
    expect(bash.result).toEqual({ stdout: "", stderr: "1 test failed" });

    expect(state.pendingTools).toEqual({});
    expect(state.lastSeq).toBe(4);
  });

  it("keeps a tool running until its result arrives", () => {
    const store = createSessionStore();
    applyAll(store, toolUse.slice(0, 1));
    const running = store.getState();
    expect(tool(running, "e1").running).toBe(true);
    expect(running.pendingTools).toEqual({ toolu_edit_fake: "e1" });
  });
});

describe("subagent fixture", () => {
  it("nests subagent output under the parent tool message", () => {
    const state = folded(subagent);

    expect(state.order).toEqual(["e1"]);
    const task = tool(state, "e1");
    expect(task.name).toBe("Task");
    expect(task.subagent).toEqual({
      description: "Survey the routes",
      agent_type: "Explore",
      is_error: false,
    });
    expect(task.children).toEqual(["e3", "e4"]);
    expect(task.running).toBe(false);
    expect(task.result).toBe("The router lives in App.tsx.");

    expect(state.subagents).toEqual({ toolu_task_fake: ["e3", "e4"] });
    const nestedText = state.messages.e3 as AssistantTextMessage;
    expect(nestedText.kind).toBe("assistant_text");
    expect(nestedText.text).toBe("Looking at the router.");
    const nestedTool = tool(state, "e4");
    expect(nestedTool.name).toBe("Read");
    expect(nestedTool.running).toBe(false);
    expect(nestedTool.result).toBe("export function App() {}");

    expect(state.pendingTools).toEqual({});
  });

  it("produces one tool message when subagent_start precedes the tool_call", () => {
    const reordered = [subagent[1], subagent[0], ...subagent.slice(2)].map(
      (event, index) => ({ ...event, seq: index + 1 }),
    );
    const state = folded(reordered);

    expect(state.order).toHaveLength(1);
    const task = tool(state, state.order[0]);
    expect(task.name).toBe("Task");
    expect(task.subagent?.description).toBe("Survey the routes");
    expect(task.children).toEqual(["e3", "e4"]);
    expect(task.running).toBe(false);
    expect(state.pendingTools).toEqual({});
  });
});

describe("stop_and_park fixture", () => {
  it("renders the signal texts and updates the loaded session state", () => {
    const store = createSessionStore();
    store.getState().setSession(fakeSession());
    const state = applyAll(store, stopAndPark);

    expect(state.order).toEqual(["e1", "e2", "e3", "e4"]);
    const stopped = state.messages.e3 as SystemMessage;
    expect(stopped.text).toBe("Stopped (SIGINT)");
    expect(stopped.level).toBe("info");
    const resumed = state.messages.e4 as SystemMessage;
    expect(resumed.text).toBe("parked → running: resumed by user");
    expect(state.session?.state).toBe("running");
  });

  it("clears turnActive when the session parks", () => {
    const state = folded(stopAndPark.slice(0, 3));
    expect(state.turnActive).toBe(false);
    expect(state.session).toBeNull();
  });

  it("renders SIGTERM as a kill", () => {
    const state = folded([
      {
        seq: 1,
        ts: "2026-01-02T13:10:00Z",
        kind: "state_change",
        from: "running",
        to: "failed",
        reason: "killed after timeout",
        signal: "SIGTERM",
      },
    ]);
    expect((state.messages.e1 as SystemMessage).text).toBe("Killed (SIGTERM)");
  });
});

describe("git_ops fixture", () => {
  it("levels git events by ok and records the cursor", () => {
    const state = folded(gitOps);

    const ok = state.messages.e1 as SystemMessage;
    expect(ok.level).toBe("info");
    expect(ok.text).toBe("Git sync succeeded");
    expect(ok.detail).toMatchObject({ ref: "sessions/fake-session" });

    const failed = state.messages.e2 as SystemMessage;
    expect(failed.level).toBe("warn");
    expect(failed.text).toBe("Git merge failed");
    expect(failed.detail).toMatchObject({ conflicts: ["src/widget.ts"] });

    expect(state.gitEventSeq).toBe(2);
  });
});

describe("raw_and_unknown fixture", () => {
  it("drops nothing: raw, orphan result, denial, warning, error, orphan child", () => {
    const state = folded(rawAndUnknown);

    expect(state.messages.e1).toMatchObject({
      kind: "raw",
      native: { type: "unknown_line", payload: 1 },
    });

    const orphan = tool(state, "e2");
    expect(orphan.name).toBe("unknown");
    expect(orphan.running).toBe(false);
    expect(orphan.result).toBe("result without a call");

    expect(state.messages.e3).toMatchObject({
      kind: "system",
      level: "warn",
      text: "Permission denied: WebFetch — not allowed by policy",
    });
    expect(state.messages.e4).toMatchObject({
      kind: "system",
      level: "warn",
      text: "secret GITHUB_TOKEN is used but not declared",
    });
    expect(state.messages.e5).toMatchObject({
      kind: "system",
      level: "error",
      text: "the CLI exited unexpectedly (fatal)",
    });
    // A fatal error ends the turn (the later nested `text` starts a new one).
    expect(folded(rawAndUnknown.slice(0, 5)).turnActive).toBe(false);

    // The never-seen parent gets a placeholder; the child never reaches `order`.
    const placeholder = tool(state, "tool:toolu_ghost_fake");
    expect(placeholder.name).toBe("Agent");
    expect(placeholder.children).toEqual(["e6"]);
    expect(state.subagents.toolu_ghost_fake).toEqual(["e6"]);
    expect(state.order).toEqual([
      "e1",
      "e2",
      "e3",
      "e4",
      "e5",
      "tool:toolu_ghost_fake",
    ]);
    expect(state.pendingTools).toEqual({ toolu_ghost_fake: "tool:toolu_ghost_fake" });
  });
});

describe("dedupe and reconnect", () => {
  it("is unchanged when the same fixture is applied twice", () => {
    const once = folded(simpleTurn);
    const store = createSessionStore();
    applyAll(store, simpleTurn);
    applyAll(store, simpleTurn);
    const twice = store.getState();

    expect(twice.order).toEqual(once.order);
    expect(twice.messages).toEqual(once.messages);
    expect(twice.lastSeq).toBe(once.lastSeq);
    expect(twice.turnActive).toBe(once.turnActive);
  });

  it("ignores a replayed overlap after a reconnect", () => {
    const eight: AgentEvent[] = Array.from({ length: 8 }, (_, index) => ({
      seq: index + 1,
      ts: "2026-01-02T16:00:00Z",
      kind: "text",
      text: `block ${index + 1}`,
    }));
    const store = createSessionStore();
    applyAll(store, eight.slice(0, 5));
    applyAll(store, eight.slice(2));
    const state = store.getState();

    expect(state.order).toHaveLength(8);
    expect(state.order).toEqual(eight.map((event) => `e${event.seq}`));
    expect(state.lastSeq).toBe(8);
  });
});

describe("prependHistory", () => {
  const older: AgentEvent[] = [
    {
      seq: 1,
      ts: "2026-01-02T17:00:00Z",
      kind: "user_message",
      text: "Fix the build",
      user_id: null,
    },
    { seq: 2, ts: "2026-01-02T17:00:01Z", kind: "text", text: "Looking." },
    { seq: 3, ts: "2026-01-02T17:00:02Z", kind: "thinking", text: "hm", redacted: false },
    { seq: 4, ts: "2026-01-02T17:00:03Z", kind: "text", text: "Found it." },
    {
      seq: 5,
      ts: "2026-01-02T17:00:04Z",
      kind: "tool_call",
      tool_use_id: "toolu_split_fake",
      name: "Bash",
      input: { command: "cargo build" },
    },
  ];
  const newer: AgentEvent[] = [
    {
      seq: 6,
      ts: "2026-01-02T17:00:05Z",
      kind: "tool_result",
      tool_use_id: "toolu_split_fake",
      content: "ok",
      is_error: false,
      truncated: false,
    },
    { seq: 7, ts: "2026-01-02T17:00:06Z", kind: "text", text: "Built." },
    {
      seq: 8,
      ts: "2026-01-02T17:00:07Z",
      kind: "result",
      subtype: "success",
      is_error: false,
      num_turns: 2,
      duration_ms: 900,
      permission_denials: [],
    },
    { seq: 9, ts: "2026-01-02T17:00:08Z", kind: "raw", backend: "claude", native: 1 },
    {
      seq: 10,
      ts: "2026-01-02T17:00:09Z",
      kind: "launch_warning",
      message: "late warning",
    },
  ];

  it("merges the older page and reconciles the split tool", () => {
    const store = createSessionStore();
    applyAll(store, newer);
    // The live half saw only the result, so it made an `unknown` tool message.
    expect(tool(store.getState(), "e6").name).toBe("unknown");

    store.getState().prependHistory(older, true);
    const state = store.getState();

    // The split tool is one message, kept at the position of its `tool_call`.
    expect(state.order).toEqual(["e1", "e2", "e3", "e4", "e5", "e7", "e8", "e9", "e10"]);
    expect(state.messages.e6).toBeUndefined();
    const merged = tool(state, "e5");
    expect(merged.name).toBe("Bash");
    expect(merged.running).toBe(false);
    expect(merged.result).toBe("ok");
    expect(state.pendingTools).toEqual({});
    expect(state.oldestSeq).toBe(1);
    expect(state.hasMore).toBe(true);
    expect(state.lastSeq).toBe(10);
  });

  it("ignores a page that is not older than the cursor", () => {
    const store = createSessionStore();
    applyAll(store, newer);
    store.getState().prependHistory(newer, false);
    const state = store.getState();

    expect(state.order).toEqual(["e6", "e7", "e8", "e9", "e10"]);
    expect(state.hasMore).toBe(false);
    expect(state.oldestSeq).toBe(6);
  });
});

describe("prependHistory across a subagent boundary", () => {
  it("reconciles the placeholder parent and a nested unknown result", () => {
    // The live half starts mid-subagent: it sees a nested result, the
    // subagent's end and the parent's result, so it invents both an `Agent`
    // parent placeholder and an `unknown` child.
    const store = createSessionStore();
    applyAll(store, subagent.slice(4));
    const live = store.getState();
    expect(tool(live, "tool:toolu_task_fake").name).toBe("Agent");
    expect(tool(live, "e5").name).toBe("unknown");

    store.getState().prependHistory(subagent.slice(0, 4), false);
    const state = store.getState();

    expect(state.order).toEqual(["e1"]);
    expect(state.messages["tool:toolu_task_fake"]).toBeUndefined();
    expect(state.messages.e5).toBeUndefined();

    const task = tool(state, "e1");
    expect(task.name).toBe("Task");
    expect(task.input).toMatchObject({ description: "Survey the routes" });
    expect(task.running).toBe(false);
    expect(task.result).toBe("The router lives in App.tsx.");
    expect(task.subagent).toEqual({
      description: "Survey the routes",
      agent_type: "Explore",
      is_error: false,
    });
    // No stale placeholder id survives in the parent's children.
    expect(task.children).toEqual(["e3", "e4"]);
    expect(state.subagents).toEqual({ toolu_task_fake: ["e3", "e4"] });

    const read = tool(state, "e4");
    expect(read.name).toBe("Read");
    expect(read.running).toBe(false);
    expect(read.result).toBe("export function App() {}");

    expect(state.pendingTools).toEqual({});
    expect(state.oldestSeq).toBe(1);
    expect(state.lastSeq).toBe(7);
  });

  it("reconciles when the cut falls between subagent_start and the children", () => {
    const store = createSessionStore();
    applyAll(store, subagent.slice(2));
    expect(store.getState().order).toEqual(["tool:toolu_task_fake"]);

    store.getState().prependHistory(subagent.slice(0, 2), false);
    const state = store.getState();

    expect(state.order).toEqual(["e1"]);
    const task = tool(state, "e1");
    expect(task.name).toBe("Task");
    expect(task.children).toEqual(["e3", "e4"]);
    expect(task.running).toBe(false);
    expect(task.subagent?.is_error).toBe(false);
    expect(tool(state, "e4").name).toBe("Read");
    expect(state.pendingTools).toEqual({});
  });

  it("leaves the parent still running when the older page is only the call", () => {
    // Only the nested result is live: the parent tool has no result yet.
    const store = createSessionStore();
    applyAll(store, subagent.slice(4, 5));
    store.getState().prependHistory(subagent.slice(0, 4), false);
    const state = store.getState();

    expect(state.order).toEqual(["e1"]);
    const task = tool(state, "e1");
    expect(task.running).toBe(true);
    expect(task.children).toEqual(["e3", "e4"]);
    expect(state.subagents).toEqual({ toolu_task_fake: ["e3", "e4"] });
    expect(state.pendingTools).toEqual({ toolu_task_fake: "e1" });
    expect(tool(state, "e4").running).toBe(false);
  });

  it("takes turnActive from the first page loaded into an empty store", () => {
    const midTurn = createSessionStore();
    midTurn.getState().prependHistory(subagent.slice(0, 2), true);
    expect(midTurn.getState().turnActive).toBe(true);

    const finished = createSessionStore();
    finished.getState().prependHistory(simpleTurn, true);
    expect(finished.getState().turnActive).toBe(false);

    // A later page never revises the live answer.
    const live = createSessionStore();
    applyAll(live, simpleTurn.slice(6));
    live.getState().prependHistory(simpleTurn.slice(0, 6), false);
    expect(live.getState().turnActive).toBe(false);
  });
});

describe("optimistic input", () => {
  it("replaces the optimistic message in place when its event arrives", () => {
    const store = createSessionStore();
    store.getState().addOptimisticUser("c-9", { kind: "message", text: "Hello" });
    store.getState().inputAccepted("c-9", 3);
    expect(store.getState().order).toEqual([optimisticId("c-9")]);
    expect(store.getState().messages[optimisticId("c-9")]).toMatchObject({
      kind: "user",
      pending: true,
    });

    store.getState().applyEvent({
      seq: 4,
      ts: "2026-01-02T18:00:00Z",
      kind: "user_message",
      text: "Hello",
      user_id: null,
      client_id: "c-9",
    });
    const state = store.getState();

    expect(state.order).toEqual(["e4"]);
    expect(state.messages[optimisticId("c-9")]).toBeUndefined();
    expect(state.messages.e4).toMatchObject({ kind: "user", text: "Hello" });
    expect((state.messages.e4 as { pending?: boolean }).pending).toBeUndefined();
  });

  it("keeps a rejected message visible so it can be resent", () => {
    const store = createSessionStore();
    store.getState().addOptimisticUser("c-x", { kind: "message", text: "Nope" });
    store.getState().inputRejected("c-x", "session is done");
    const message = store.getState().messages[optimisticId("c-x")];

    expect(message).toMatchObject({
      kind: "user",
      text: "Nope",
      pending: false,
      rejected: "session is done",
    });
    expect(store.getState().order).toEqual([optimisticId("c-x")]);
  });
});

describe("store lifecycle", () => {
  it("reset clears the folded state and every cursor", () => {
    const store = createSessionStore();
    store.getState().setSession(fakeSession());
    store.getState().setStatus("live");
    applyAll(store, gitOps);
    applyAll(store, simpleTurn.slice(0, 3));
    store.getState().prependHistory([], true);
    store.getState().reset();
    const state = store.getState();

    expect(state).toMatchObject({
      session: null,
      status: "connecting",
      lastSeq: 0,
      order: [],
      messages: {},
      pendingTools: {},
      subagents: {},
      oldestSeq: null,
      hasMore: false,
      gitEventSeq: 0,
      turnActive: false,
    });
  });

  it("gives each store its own state", () => {
    const first = createSessionStore();
    const second = createSessionStore();
    applyAll(first, simpleTurn);

    expect(second.getState().order).toEqual([]);
    expect(second.getState().lastSeq).toBe(0);
  });
});
