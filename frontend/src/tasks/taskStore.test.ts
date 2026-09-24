// The refresh ordering of `SPEC.md`, "Frontend", "Board refresh ordering"
// (ADR 0022), driven through a store whose two reads are deferred promises, so
// every interleaving an event can have with a load in flight is exact.

import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";

import * as services from "../services/tasks";
import * as taskStateServices from "../services/taskStates";

// Only the two reads `createQueryDeps` makes; the rest of the store's tests
// inject their own deps and never reach the services at all.
vi.mock("../services/tasks", async (original) => ({
  ...(await original<typeof services>()),
  listTasks: vi.fn(),
}));

vi.mock("../services/taskStates", async (original) => ({
  ...(await original<typeof taskStateServices>()),
  listTaskStates: vi.fn(),
}));

import { ApiError } from "../services/apiClient";
import { signOut } from "../services/auth";
import type { Task, TaskEvent, TaskState } from "../types";
import { taskKeys } from "./queryKeys";
import {
  createQueryDeps,
  createTaskStore,
  selectColumns,
  selectTaskById,
  selectVisibleColumns,
  UNKNOWN_COLUMN,
  useTaskStore,
  type TaskStoreDeps,
} from "./taskStore";

const listTasks = vi.mocked(services.listTasks);
const listTaskStates = vi.mocked(taskStateServices.listTaskStates);

afterEach(() => {
  vi.clearAllMocks();
});

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
      stateCalls[index]?.resolve(states);
      taskCalls[index]?.resolve(tasks);
      await flush();
    },
    failBoth: async (index, error) => {
      stateCalls[index]?.reject(error);
      taskCalls[index]?.reject(error);
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
    auto_merge: false,
    conflict_state: null,
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
    rounds: 0,
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
  it("installs responses dirtied by an event and coalesces one follow-up", async () => {
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
    // Older than the event, newer than the empty board it replaces: it is
    // installed, and the event is still owed its own read.
    expect(store.getState().loaded).toBe(true);
    expect(store.getState().tasks).toHaveLength(1);
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
    expect(selectColumns(store.getState()).map((c) => c.name)).toEqual([
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
    await deps.failBoth(1, new ApiError(503, "network down"));

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
    // A mutation's own refresh is a whole snapshot too: it is shown, and the
    // event the write is about to produce still gets its own read.
    expect(store.getState().loaded).toBe(true);
    expect(store.getState().tasks.map((t) => t.number)).toEqual([1]);
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

describe("the snapshot's index", () => {
  it("resolves a task by id without scanning the board", async () => {
    const deps = harness();
    const store = createTaskStore(deps);
    store.getState().bindProject(PROJECT);

    void store.getState().refresh();
    await deps.settle(0, [READY], [task(1, "ready"), task(2, "ready")]);

    expect(selectTaskById("task-2")(store.getState())?.number).toBe(2);
    expect(selectTaskById("task-9")(store.getState())).toBeUndefined();
  });

  it("describe the snapshot that is installed, never the one before it", async () => {
    const deps = harness();
    const store = createTaskStore(deps);
    store.getState().bindProject(PROJECT);

    void store.getState().refresh();
    await deps.settle(0, [READY], [task(1, "ready")]);
    store.getState().invalidate();
    await deps.settle(1, [READY], [task(2, "ready")]);

    expect(selectTaskById("task-1")(store.getState())).toBeUndefined();
    expect(selectTaskById("task-2")(store.getState())?.number).toBe(2);

    // A project change empties them with the arrays they index.
    store.getState().bindProject(OTHER_PROJECT);
    expect(selectTaskById("task-2")(store.getState())).toBeUndefined();
  });
});

describe("the board's reads through the query cache", () => {
  /** A query client with retries off: a rejected read is the read's answer. */
  function client(): QueryClient {
    return new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
  }

  it("never installs a read that began before the refresh did", async () => {
    // The ordering `fetchQuery` alone could not give: it joins a request
    // already in flight for the same key, and that request may have been
    // started before the event this refresh is answering.
    const cache = client();
    const store = createTaskStore(createQueryDeps(cache));
    const responses: Deferred<Task[]>[] = [];
    listTasks.mockImplementation(() => {
      const next = deferred<Task[]>();
      responses.push(next);
      return next.promise;
    });
    listTaskStates.mockResolvedValue([READY]);

    // A read of the same key, in flight before the board asks for anything.
    cache
      .fetchQuery({
        queryKey: taskKeys.all(PROJECT),
        queryFn: () => services.listTasks(PROJECT),
      })
      // Whoever started it is told it was cancelled; here nobody is waiting.
      .catch(() => undefined);
    await flush();
    expect(listTasks).toHaveBeenCalledTimes(1);

    store.getState().bindProject(PROJECT);
    void store.getState().refresh();
    await flush();

    // The refresh made a request of its own rather than joining that one.
    expect(listTasks).toHaveBeenCalledTimes(2);

    // The older read answers first, and with the older board.
    responses[0]?.resolve([task(1, "ready")]);
    await flush();
    expect(store.getState().loaded).toBe(false);

    responses[1]?.resolve([task(1, "ready"), task(2, "ready")]);
    await flush();
    expect(store.getState().tasks.map((t) => t.number)).toEqual([1, 2]);
  });

  it("keeps the identity of a task that did not change", async () => {
    // `fetchQuery` resolves with the response; the cache holds the
    // structurally shared copy, and that is what `memo(TaskCard)` compares.
    const cache = client();
    const deps = createQueryDeps(cache);
    listTaskStates.mockResolvedValue([READY]);
    // A fresh object graph every time, as a real response is.
    listTasks.mockImplementation(() =>
      Promise.resolve([task(1, "ready"), task(2, "ready")]),
    );

    const first = await deps.listTasks(PROJECT);
    const second = await deps.listTasks(PROJECT);

    expect(second[0]).toBe(first[0]);
    expect(second[1]).toBe(first[1]);
  });
});

describe("selectColumns", () => {
  it("orders columns by position and keeps the API's task order", () => {
    const columns = selectColumns({
      states: [BACKLOG, READY],
      tasks: [task(9, "ready"), task(2, "backlog"), task(4, "ready")],
    });

    expect(columns.map((c) => c.name)).toEqual(["backlog", "ready"]);
    expect(columns.map((c) => c.key)).toEqual(["state-backlog", "state-ready"]);
    expect(columns[0]?.state).toBe(BACKLOG);
    // The API orders by priority, then number; the column does not re-sort.
    expect(columns[1]?.tasks.map((t) => t.number)).toEqual([9, 4]);
  });

  it("has no unknown column while every task is in a current state", () => {
    const columns = selectColumns({
      states: [BACKLOG],
      tasks: [task(1, "backlog")],
    });

    expect(columns).toHaveLength(1);
  });

  it("buckets a task in no current state into a trailing unknown column", () => {
    const columns = selectColumns({
      states: [BACKLOG, READY],
      // Mid-refresh: `ready` was renamed and this task still names the old one.
      tasks: [task(1, "backlog"), task(2, "was-ready"), task(3, "was-ready")],
    });

    expect(columns.map((c) => c.name)).toEqual([
      "backlog",
      "ready",
      UNKNOWN_COLUMN,
    ]);
    const unknown = columns[columns.length - 1];
    // Not a fabricated state row: nothing can mistake it for a real column.
    expect(unknown?.state).toBeNull();
    expect(unknown?.key).toBe(UNKNOWN_COLUMN);
    expect(unknown?.tasks.map((t) => t.number)).toEqual([2, 3]);
  });

  it("keeps an empty board's columns", () => {
    const columns = selectColumns({ states: [BACKLOG, READY], tasks: [] });

    expect(columns).toHaveLength(2);
    expect(columns.every((c) => c.tasks.length === 0)).toBe(true);
  });
});

describe("selectVisibleColumns", () => {
  function titled(number: number, title: string, stateName: string): Task {
    return { ...task(number, stateName), title };
  }

  const SNAPSHOT = {
    states: [BACKLOG, READY],
    tasks: [
      titled(42, "Fix Login Redirect", "ready"),
      titled(142, "Rotate the deploy key", "backlog"),
      titled(7, "Ship the login banner", "ready"),
    ],
  };

  it("is selectColumns itself while the query is empty", () => {
    const visible = selectVisibleColumns({ ...SNAPSHOT, query: "  " });

    expect(visible.map((c) => c.name)).toEqual(["backlog", "ready"]);
    expect(visible.map((c) => c.tasks.map((t) => t.number))).toEqual([
      [142],
      [42, 7],
    ]);
  });

  it("filters each column and keeps every column and its order", () => {
    const visible = selectVisibleColumns({ ...SNAPSHOT, query: "login" });

    expect(visible.map((c) => c.name)).toEqual(["backlog", "ready"]);
    expect(visible[0]?.tasks).toEqual([]);
    expect(visible[1]?.tasks.map((t) => t.number)).toEqual([42, 7]);
  });

  it("matches an exact number across all columns", () => {
    const visible = selectVisibleColumns({ ...SNAPSHOT, query: "#142" });

    expect(visible.map((c) => c.tasks.map((t) => t.number))).toEqual([
      [142],
      [],
    ]);
  });

  it("leaves the snapshot arrays untouched", () => {
    const states = [...SNAPSHOT.states];
    const tasks = [...SNAPSHOT.tasks];
    selectVisibleColumns({ states, tasks, query: "login" });

    expect(states).toEqual(SNAPSHOT.states);
    expect(tasks).toEqual(SNAPSHOT.tasks);
  });

  it("keeps an unknown bucket the query emptied", () => {
    const visible = selectVisibleColumns({
      states: [BACKLOG],
      tasks: [titled(1, "Fix Login Redirect", "was-ready")],
      query: "deploy",
    });

    // The bucket is decided by the unfiltered snapshot: a column that came and
    // went with the query would read as a state appearing mid-search.
    expect(visible.map((c) => c.name)).toEqual(["backlog", UNKNOWN_COLUMN]);
    expect(visible.every((c) => c.tasks.length === 0)).toBe(true);
  });
});

describe("the board's query", () => {
  it("setQuery reads nothing: no listTasks, no listTaskStates", async () => {
    const deps = harness();
    const store = createTaskStore(deps);
    store.getState().bindProject(PROJECT);
    void store.getState().refresh();
    await deps.settle(0, [READY], [task(1, "ready")]);

    store.getState().setQuery("log");
    store.getState().setQuery("login");
    store.getState().setQuery("");
    await flush();

    expect(store.getState().query).toBe("");
    // The one pair of reads is the load above; typing added none.
    expect(deps.taskCalls).toHaveLength(1);
    expect(deps.stateCalls).toHaveLength(1);
    expect(store.getState().loading).toBe(false);
  });

  it("survives an event-triggered refresh", async () => {
    const deps = harness();
    const store = createTaskStore(deps);
    store.getState().bindProject(PROJECT);
    void store.getState().refresh();
    await deps.settle(0, [READY], [task(1, "ready")]);

    store.getState().setQuery("login");
    expect(store.getState().noteEvent(event(1))).toBe(true);
    await deps.settle(1, [READY], [task(1, "ready"), task(2, "ready")]);

    expect(store.getState().query).toBe("login");
    expect(store.getState().tasks).toHaveLength(2);
  });

  it("is cleared by binding another project", () => {
    const deps = harness();
    const store = createTaskStore(deps);
    store.getState().bindProject(PROJECT);
    store.getState().setQuery("login");

    store.getState().bindProject(OTHER_PROJECT);
    expect(store.getState().query).toBe("");
  });
});

// The rule of `SPEC.md`, "Frontend", Rules — a sign-out "clears authenticated
// query and stream stores" — reaches the board through a handler this module
// registers at import, not through `AuthBootstrap`, so the entry chunk carries
// no edge to the board's fold (`ARCHITECTURE.md`, "Frontend architecture",
// Barrels and the first-paint path). The registration is therefore asserted
// through the shared `useTaskStore`, which is the store it was made for.
describe("the shared board store on sign-out", () => {
  it("drops the snapshot the signed-out user was looking at", async () => {
    listTaskStates.mockResolvedValue([READY]);
    listTasks.mockResolvedValue([task(1, "ready")]);

    useTaskStore.getState().bindProject(PROJECT);
    await useTaskStore.getState().refresh();
    useTaskStore.getState().setQuery("login");
    expect(useTaskStore.getState().tasks).toHaveLength(1);

    signOut("user");

    expect(useTaskStore.getState().projectId).toBeNull();
    expect(useTaskStore.getState().tasks).toHaveLength(0);
    expect(useTaskStore.getState().loaded).toBe(false);
    expect(useTaskStore.getState().query).toBe("");
  });
});
