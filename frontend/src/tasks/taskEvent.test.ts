// The task stream's frame boundary (`SPEC.md`, "SSE: task stream",
// "TaskEvent"): what is accepted, and what an off-contract frame is kept from
// doing to the board's cursor.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as projects from "../services/projects";
import * as tasks from "../services/tasks";
import * as taskStates from "../services/taskStates";
import { parseTaskEvent, validateTaskEvent } from "./taskEvent";
import { useTaskStore } from "./taskStore";
import type { EventSourceLike } from "./useTaskStream";
import { TaskStream } from "./useTaskStream";

vi.mock("../services/auth", () => ({
  getAccessToken: vi.fn(() => "token-one"),
  refreshAccessToken: vi.fn(),
  onCredentialsReplaced: vi.fn(() => () => {}),
  onSignOut: vi.fn(() => () => {}),
  isStaleRefreshError: vi.fn(() => false),
}));

vi.mock("../services/tasks", async (original) => ({
  ...(await original<typeof tasks>()),
  listTasks: vi.fn(),
}));

vi.mock("../services/taskStates", async (original) => ({
  ...(await original<typeof taskStates>()),
  listTaskStates: vi.fn(),
}));

vi.mock("../services/projects", async (original) => ({
  ...(await original<typeof projects>()),
  getProject: vi.fn(),
}));

const PROJECT = "22222222-2222-4222-8222-222222222222";

function event(
  overrides: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    seq: 1,
    ts: "2026-09-21T10:00:00Z",
    task_id: "44444444-4444-4444-8444-444444444444",
    actor: { kind: "system" },
    kind: "updated",
    ...overrides,
  };
}

describe("validateTaskEvent", () => {
  it("accepts a well-formed event", () => {
    expect(validateTaskEvent(event())).not.toBeNull();
  });

  it("accepts a states_changed event, whose task_id is null", () => {
    expect(
      validateTaskEvent(event({ kind: "states_changed", task_id: null })),
    ).not.toBeNull();
  });

  it("rejects a seq that is not a sequence", () => {
    // The failure this guard exists for: `undefined <= lastSeq` is false, so
    // the board would take `undefined` as its cursor and ask the server for
    // `?after=undefined` on every reconnect for ever.
    expect(validateTaskEvent(event({ seq: undefined }))).toBeNull();
    expect(validateTaskEvent(event({ seq: null }))).toBeNull();
    expect(validateTaskEvent(event({ seq: "12" }))).toBeNull();
    expect(validateTaskEvent(event({ seq: 2.5 }))).toBeNull();
    expect(validateTaskEvent(event({ seq: 0 }))).toBeNull();
    expect(validateTaskEvent(event({ seq: Number.NaN }))).toBeNull();
  });

  it("rejects an envelope field of the wrong type", () => {
    expect(validateTaskEvent(event({ ts: 17 }))).toBeNull();
    expect(validateTaskEvent(event({ kind: null }))).toBeNull();
    expect(validateTaskEvent(event({ task_id: 4 }))).toBeNull();
    expect(validateTaskEvent(event({ actor: null }))).toBeNull();
    expect(validateTaskEvent(event({ actor: {} }))).toBeNull();
    expect(validateTaskEvent(event({ actor: "user" }))).toBeNull();
  });

  it("rejects a non-object", () => {
    expect(validateTaskEvent(null)).toBeNull();
    expect(validateTaskEvent([event()])).toBeNull();
    expect(validateTaskEvent(3)).toBeNull();
  });

  it("accepts a kind it has never heard of", () => {
    // Every kind means the same thing to the board — refresh — so a kind a
    // newer orchestrator added is already handled by being counted.
    expect(validateTaskEvent(event({ kind: "archived" }))).not.toBeNull();
  });

  it("rejects text that is not JSON", () => {
    expect(parseTaskEvent("{oops")).toBeNull();
    expect(parseTaskEvent("")).toBeNull();
  });

  it("parses a well-formed frame", () => {
    expect(parseTaskEvent(JSON.stringify(event({ seq: 12 })))?.seq).toBe(12);
  });
});

describe("TaskStream frame handling", () => {
  class FakeSource implements EventSourceLike {
    static instances: FakeSource[] = [];

    readonly url: string;
    onopen: ((ev: Event) => void) | null = null;
    onerror: ((ev: Event) => void) | null = null;
    private listener: ((ev: MessageEvent<string>) => void) | null = null;

    constructor(url: string) {
      this.url = url;
      FakeSource.instances.push(this);
    }

    close(): void {}

    addEventListener(
      _type: "task",
      listener: (ev: MessageEvent<string>) => void,
    ): void {
      this.listener = listener;
    }

    open(): void {
      this.onopen?.(new Event("open"));
    }

    deliver(data: string): void {
      this.listener?.(new MessageEvent("task", { data }));
    }
  }

  let stream: TaskStream;
  let warn: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    FakeSource.instances = [];
    vi.mocked(tasks.listTasks).mockResolvedValue([]);
    vi.mocked(taskStates.listTaskStates).mockResolvedValue([]);
    warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    stream = new TaskStream(PROJECT, (url) => new FakeSource(url));
    stream.start();
    FakeSource.instances[0]?.open();
  });

  afterEach(() => {
    stream.dispose();
    useTaskStore.getState().reset();
    vi.restoreAllMocks();
  });

  it("never lets an off-contract frame become the cursor", () => {
    const source = FakeSource.instances[0];
    source?.deliver("{oops");
    source?.deliver(JSON.stringify(event({ seq: undefined })));
    source?.deliver(JSON.stringify(event({ seq: "9" })));
    source?.deliver(JSON.stringify(event({ actor: null })));
    expect(useTaskStore.getState().lastSeq).toBe(0);
    expect(warn).toHaveBeenCalled();
  });

  it("reopens with the last good cursor after a bad frame", () => {
    const source = FakeSource.instances[0];
    source?.deliver(JSON.stringify(event({ seq: 5 })));
    source?.deliver(JSON.stringify(event({ seq: undefined })));
    expect(useTaskStore.getState().lastSeq).toBe(5);
    stream.reconnect();
    expect(FakeSource.instances[1]?.url).toContain("after=5");
    expect(FakeSource.instances[1]?.url).not.toContain("undefined");
  });

  it("counts a valid event, including one of an unknown kind", () => {
    const source = FakeSource.instances[0];
    source?.deliver(JSON.stringify(event({ seq: 2, kind: "archived" })));
    expect(useTaskStore.getState().lastSeq).toBe(2);
  });
});
