// The refresh ordering of `SPEC.md`, "Frontend", "Board refresh ordering"
// (ADR 0022), driven through a store whose two reads are deferred promises, so
// every interleaving an event can have with a load in flight is exact.

import { describe, expect, it } from "vitest";

import type { Task, TaskEvent, TaskState } from "../types";
import { createTaskStore, selectColumns, type TaskStoreDeps } from "./taskStore";

const PROJECT = "11111111-1111-4111-8111-111111111111";
const OTHER_PROJECT = "22222222-2222-4222-8222-222222222222";

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Deps that hand out one deferred per call, in call order. */
interface Harness extends TaskStoreDeps {
  taskCalls: Deferred<Task[]>[];
  stateCalls: Deferred<TaskState[]>[];
  projects: string[];
  /** Settles the nth pair of reads with the given snapshot. */
  settle: (index: number, states: TaskState[], tasks: Task[]) => Promise<void>;
  failBoth: (index: number, error: unknown) => Promise<void>;
}

function harness(): Harness {
  const taskCalls: Deferred<Task[]>[] = [];
  const stateCalls: Deferred<TaskState[]>[] = [];
  const projects: string[] = [];
  return {
    taskCalls,
    stateCalls,
    projects,
    listTasks: (projectId) => {
      projects.push(projectId);
      const next = deferred<Task[]>();
      taskCalls.push(next);
      return next.promise;
    },
    listTaskStates: () => {
      const next = deferred<TaskState[]>();
      stateCalls.push(next);
      return next.promise;
    },
    settle: async (index, states, tasks) => {
      stateCalls[index].resolve(states);
      taskCalls[index].resolve(tasks);
      await flush();
    },
    failBoth: async (index, error) => {
      stateCalls[index].reject(error);
      taskCalls[index].reject(error);
      await flush();
    },
  };
}

/** Lets every already-queued microtask (the awaits inside `refresh`) run. */
async function flush(): Promise<void> {
  for (let i = 0; i < 5; i += 1) await Promise.resolve();
}

function state(name: string, position: number): TaskState {
  return {
    id: `state-${name}`,
    project_id: PROJECT,
    name,
    kind: "queue",
    position,
    created_at: "2026-01-01T00:00:00Z",
  };
}

function task(number: number, stateName: string): Task {
  return {
    id: `task-${number}`,
    project_id: PROJECT,
    number,
    title: `Task ${number}`,
    description: null,
    state: stateName,
    priority: 2,
    blocked: false,
    labels: [],
    parent_id: null,
    assignee_user_id: null,
    lease_holder_session_id: null,
    lease_since: null,
    attempts: 0,
    needs_human_reason: null,
    handoff: null,
    depends_on: [],
    blocks: [],
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    closed_at: null,
  };
}

function event(seq: number): TaskEvent {
  return {
    seq,
    ts: "2026-01-01T00:00:00Z",
    task_id: "task-1",
    actor: { kind: "system" },
    kind: "updated",
  };
}

const READY = state("ready", 1);
const BACKLOG = state("backlog", 0);

describe("taskStore refresh ordering", () => {
  it("discards responses dirtied by an event and coalesces one follow-up", async () => {
    const deps = harness();
    const store = createTaskStore(deps);
    store.getState().bindProject(PROJECT);

    void store.getState().refresh();
    expect(deps.taskCalls).toHaveLength(1);

    // An event lands while both reads are outstanding.
    expect(store.getState().noteEvent(event(7))).toBe(true);
    expect(deps.taskCalls).toHaveLength(1);
    expect(store.getState().dirty).toBe(true);

    await deps.settle(0, [READY], [task(1, "ready")]);
    // The dirtied responses are dropped, not installed.
    expect(store.getState().loaded).toBe(false);
    expect(store.getState().tasks).toEqual([]);
    // Exactly one follow-up, per endpoint.
    expect(deps.taskCalls).toHaveLength(2);
    expect(deps.stateCalls).toHaveLength(2);

    await deps.settle(1, [READY, BACKLOG], [task(1, "ready")]);
    expect(deps.taskCalls).toHaveLength(2);
    expect(store.getState().loaded).toBe(true);
    expect(store.getState().dirty).toBe(false);
    expect(store.getState().lastSeq).toBe(7);
    // States are stored in `position` order whatever the response order.
    expect(store.getState().states.map((s) => s.name)).toEqual([
      "backlog",
      "ready",
    ]);
    expect(selectColumns(store.getState()).map((c) => c.state.name)).toEqual([
      "backlog",
      "ready",
    ]);
  });

  it("keeps the previous snapshot when a read fails", async () => {
    const deps = harness();
    const store = createTaskStore(deps);
    store.getState().bindProject(PROJECT);

    void store.getState().refresh();
    await deps.settle(0, [READY], [task(1, "ready")]);
    expect(store.getState().tasks).toHaveLength(1);

    store.getState().invalidate();
    await deps.failBoth(1, new Error("network down"));

    expect(store.getState().tasks).toHaveLength(1);
    expect(store.getState().states).toHaveLength(1);
    expect(store.getState().loaded).toBe(true);
    expect(store.getState().lastSeq).toBe(0);
    expect(store.getState().dirty).toBe(true);
    expect(store.getState().error).toBe("network down");
    expect(store.getState().loading).toBe(false);
    // A failed follow-up does not retry by itself.
    expect(deps.taskCalls).toHaveLength(2);
  });

  it("ignores a duplicate seq and refreshes nothing", async () => {
    const deps = harness();
    const store = createTaskStore(deps);
    store.getState().bindProject(PROJECT);

    void store.getState().refresh();
    await deps.settle(0, [READY], []);
    expect(store.getState().noteEvent(event(4))).toBe(true);
    await deps.settle(1, [READY], []);

    const before = store.getState();
    expect(store.getState().noteEvent(event(4))).toBe(false);
    expect(store.getState().noteEvent(event(3))).toBe(false);
    await flush();

    expect(deps.taskCalls).toHaveLength(2);
    expect(store.getState().eventGeneration).toBe(before.eventGeneration);
    expect(store.getState().dirty).toBe(false);
    expect(store.getState().lastSeq).toBe(4);
  });

  it("never installs the responses of a project that has been left", async () => {
    const deps = harness();
    const store = createTaskStore(deps);
    store.getState().bindProject(PROJECT);

    void store.getState().refresh();
    store.getState().bindProject(OTHER_PROJECT);
    // The new view asks for its own load while the old reads are outstanding.
    void store.getState().refresh();
    expect(deps.taskCalls).toHaveLength(1);

    await deps.settle(0, [READY], [task(1, "ready")]);
    expect(store.getState().tasks).toEqual([]);
    expect(store.getState().loaded).toBe(false);
    // The new view's load runs once the stale one has been discarded.
    expect(deps.taskCalls).toHaveLength(2);
    expect(deps.projects[1]).toBe(OTHER_PROJECT);

    await deps.settle(1, [BACKLOG], [task(9, "backlog")]);
    expect(store.getState().projectId).toBe(OTHER_PROJECT);
    expect(store.getState().tasks.map((t) => t.number)).toEqual([9]);
  });

  it("does not start a second concurrent read", async () => {
    const deps = harness();
    const store = createTaskStore(deps);
    store.getState().bindProject(PROJECT);

    void store.getState().refresh();
    void store.getState().refresh();
    void store.getState().refresh();
    await flush();
    expect(deps.taskCalls).toHaveLength(1);
    expect(deps.stateCalls).toHaveLength(1);
    expect(store.getState().loading).toBe(true);

    await deps.settle(0, [READY], []);
    // The requests made while loading are coalesced into one follow-up.
    expect(deps.taskCalls).toHaveLength(2);
    expect(store.getState().loading).toBe(true);

    await deps.settle(1, [READY], []);
    expect(deps.taskCalls).toHaveLength(2);
    expect(store.getState().loading).toBe(false);
  });

  it("performs one follow-up for an invalidate during a load", async () => {
    const deps = harness();
    const store = createTaskStore(deps);
    store.getState().bindProject(PROJECT);

    void store.getState().refresh();
    store.getState().invalidate();
    expect(deps.taskCalls).toHaveLength(1);

    await deps.settle(0, [READY], [task(1, "ready")]);
    expect(store.getState().loaded).toBe(false);
    expect(deps.taskCalls).toHaveLength(2);

    await deps.settle(1, [READY], [task(2, "ready")]);
    expect(deps.taskCalls).toHaveLength(2);
    expect(store.getState().tasks.map((t) => t.number)).toEqual([2]);
    expect(store.getState().dirty).toBe(false);
    expect(store.getState().error).toBeNull();
  });

  it("bumps the view on reset so an in-flight read is discarded", async () => {
    const deps = harness();
    const store = createTaskStore(deps);
    store.getState().bindProject(PROJECT);
    store.getState().setQuery("login");
    store.getState().setStream("live");

    void store.getState().refresh();
    const view = store.getState().viewGeneration;
    store.getState().reset();
    expect(store.getState().viewGeneration).toBe(view + 1);
    expect(store.getState().projectId).toBeNull();
    expect(store.getState().query).toBe("");
    expect(store.getState().stream).toBe("connecting");

    await deps.settle(0, [READY], [task(1, "ready")]);
    expect(store.getState().tasks).toEqual([]);
    expect(store.getState().loaded).toBe(false);
    expect(deps.taskCalls).toHaveLength(1);
  });

  it("binding the same project again changes nothing", () => {
    const deps = harness();
    const store = createTaskStore(deps);
    store.getState().bindProject(PROJECT);
    store.getState().setQuery("login");
    const view = store.getState().viewGeneration;

    store.getState().bindProject(PROJECT);
    expect(store.getState().viewGeneration).toBe(view);
    expect(store.getState().query).toBe("login");
  });
});
