// The stream half of the task board (`SPEC.md`, "SSE: task stream" and the
// stream rules of "Authentication"): open before load, one refresh per open,
// invalidation on every fresh event, and an explicit reconnect with a
// refreshed token. A fake `EventSource` on `globalThis` stands in for the
// browser's, which never retries on its own here.

import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { queryClient } from "../queryClient";
import { ApiError } from "../services/apiClient";
import * as auth from "../services/auth";
import * as tasks from "../services/tasks";
import * as taskStates from "../services/taskStates";
import type { AuthResponse, Task, TaskEvent, TaskState } from "../types";
import { taskKeys } from "./queryKeys";
import { useTaskStore } from "./taskStore";
import { useTaskStream } from "./useTaskStream";

vi.mock("../services/auth", async (original) => {
  const actual = await original<typeof auth>();
  return {
    getAccessToken: vi.fn(() => "token-one"),
    refreshAccessToken: vi.fn(),
    onCredentialsReplaced: vi.fn(() => () => {}),
    onSignOut: vi.fn(() => () => {}),
    // The stale-completion marker is a value, not a collaborator: the stream
    // has to recognise the real one.
    StaleRefreshError: actual.StaleRefreshError,
    isStaleRefreshError: actual.isStaleRefreshError,
  };
});

vi.mock("../services/tasks", async (original) => ({
  ...(await original<typeof tasks>()),
  listTasks: vi.fn(),
}));

vi.mock("../services/taskStates", async (original) => ({
  ...(await original<typeof taskStates>()),
  listTaskStates: vi.fn(),
}));

const getAccessToken = vi.mocked(auth.getAccessToken);
const refreshAccessToken = vi.mocked(auth.refreshAccessToken);
const onCredentialsReplaced = vi.mocked(auth.onCredentialsReplaced);
const listTasks = vi.mocked(tasks.listTasks);
const listTaskStates = vi.mocked(taskStates.listTaskStates);

const PROJECT = "11111111-1111-4111-8111-111111111111";

/** The refresh only has to resolve; the new token comes from `getAccessToken`. */
const REFRESHED = {} as AuthResponse;

class FakeEventSource {
  static instances: FakeEventSource[] = [];

  readonly url: string;
  closed = 0;
  onopen: ((ev: Event) => void) | null = null;
  onerror: ((ev: Event) => void) | null = null;
  private readonly listeners = new Map<
    string,
    ((ev: MessageEvent<string>) => void)[]
  >();

  constructor(url: string) {
    this.url = url;
    FakeEventSource.instances.push(this);
  }

  addEventListener(
    type: string,
    listener: (ev: MessageEvent<string>) => void,
  ): void {
    const existing = this.listeners.get(type) ?? [];
    existing.push(listener);
    this.listeners.set(type, existing);
  }

  close(): void {
    this.closed += 1;
  }

  /** The server's response headers arriving. */
  accept(): void {
    this.onopen?.(new Event("open"));
  }

  /** One `event: task` message. */
  task(data: string): void {
    for (const listener of this.listeners.get("task") ?? []) {
      listener(new MessageEvent<string>("task", { data }));
    }
  }

  fail(): void {
    this.onerror?.(new Event("error"));
  }
}

function last(): FakeEventSource {
  const source = FakeEventSource.instances.at(-1);
  if (!source) throw new Error("no stream was opened");
  return source;
}

function state(name: string, position: number): TaskState {
  return {
    id: `state-${position}`,
    project_id: PROJECT,
    name,
    position,
    kind: "queue",
    created_at: "2026-01-01T00:00:00Z",
  };
}

function taskEvent(seq: number, overrides: Partial<TaskEvent> = {}): string {
  return JSON.stringify({
    seq,
    ts: "2026-01-01T00:00:00Z",
    task_id: "33333333-3333-4333-8333-333333333333",
    actor: { kind: "system" },
    kind: "updated",
    ...overrides,
  } satisfies TaskEvent);
}

/** Lets the promise chains inside `refresh` and the reconnect settle. */
async function settle(): Promise<void> {
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
}

/**
 * Lets the coalesced invalidation fall due. Real timers only — every test that
 * calls this asserts on `invalidateQueries`, and none of them fakes time.
 */
async function tick(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await settle();
}

beforeEach(() => {
  FakeEventSource.instances = [];
  vi.stubGlobal("EventSource", FakeEventSource);
  vi.clearAllMocks();
  getAccessToken.mockReturnValue("token-one");
  listTasks.mockResolvedValue([] as Task[]);
  listTaskStates.mockResolvedValue([state("Ready", 0)]);
  queryClient.clear();
  useTaskStore.getState().reset();
  vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function board() {
  return useTaskStore.getState();
}

describe("useTaskStream", () => {
  it("opens with the cursor and loads nothing before the stream opens", async () => {
    const view = renderHook(() => useTaskStream(PROJECT));

    expect(FakeEventSource.instances).toHaveLength(1);
    expect(last().url).toBe(
      `/api/projects/${PROJECT}/tasks/stream?token=token-one&after=latest`,
    );
    expect(board().projectId).toBe(PROJECT);
    expect(listTasks).not.toHaveBeenCalled();
    expect(listTaskStates).not.toHaveBeenCalled();

    await act(async () => {
      last().accept();
      await settle();
    });

    expect(listTasks).toHaveBeenCalledTimes(1);
    expect(listTaskStates).toHaveBeenCalledTimes(1);
    expect(board().stream).toBe("live");
    expect(view.result.current).toBe("live");

    view.unmount();
  });

  it("notes a task event, refreshes and invalidates the task keys", async () => {
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");
    const view = renderHook(() => useTaskStream(PROJECT));
    await act(async () => {
      last().accept();
      await settle();
    });

    await act(async () => {
      last().task(taskEvent(4));
      await tick();
    });

    expect(board().lastSeq).toBe(4);
    expect(listTasks).toHaveBeenCalledTimes(2);
    expect(invalidate).toHaveBeenCalledWith({ queryKey: taskKeys.all(PROJECT) });

    // A replayed sequence changes nothing and owes no refresh.
    await act(async () => {
      last().task(taskEvent(4));
      await settle();
    });
    expect(listTasks).toHaveBeenCalledTimes(2);

    // Malformed JSON is logged and ignored.
    await act(async () => {
      last().task("{not json");
      await settle();
    });
    expect(board().lastSeq).toBe(4);

    view.unmount();
  });

  it("invalidates the task keys for a states_changed that carries no task_id", async () => {
    // A state rename emits only this, and an open drawer is showing the old
    // name until the keys under `taskKeys.all` are invalidated.
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");
    const view = renderHook(() => useTaskStream(PROJECT));
    await act(async () => {
      last().accept();
      await settle();
    });

    await act(async () => {
      last().task(taskEvent(1, { kind: "states_changed", task_id: null }));
      await tick();
    });

    expect(invalidate).toHaveBeenCalledWith({ queryKey: taskKeys.all(PROJECT) });
    expect(listTasks).toHaveBeenCalledTimes(2);

    view.unmount();
  });

  it("coalesces a burst of events into one invalidation", async () => {
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");
    const view = renderHook(() => useTaskStream(PROJECT));
    await act(async () => {
      last().accept();
      await settle();
    });
    invalidate.mockClear();

    await act(async () => {
      for (let seq = 1; seq <= 20; seq += 1) last().task(taskEvent(seq));
      await tick();
    });

    // Each one would otherwise be a scan of the query cache and a refetch of
    // every open task detail.
    expect(invalidate).toHaveBeenCalledTimes(1);
    expect(board().lastSeq).toBe(20);

    view.unmount();
  });

  it("invalidates nothing under the sessions keys", async () => {
    // The per-session task lists are refreshed where an observer exists — the
    // session page's own panel — not from a stream that is mounted only while
    // the board is.
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");
    const view = renderHook(() => useTaskStream(PROJECT));
    await act(async () => {
      last().accept();
      await settle();
    });

    await act(async () => {
      last().task(taskEvent(1, { kind: "claimed" }));
      await tick();
    });

    for (const [filters] of invalidate.mock.calls) {
      expect(filters?.predicate).toBeUndefined();
      expect(filters?.queryKey).toEqual(taskKeys.all(PROJECT));
    }

    view.unmount();
  });

  it("closes, refreshes the token and reopens at the last sequence", async () => {
    vi.useFakeTimers();
    vi.spyOn(Math, "random").mockReturnValue(0.5); // no jitter
    refreshAccessToken.mockResolvedValue(REFRESHED);
    const view = renderHook(() => useTaskStream(PROJECT));
    await act(async () => {
      last().accept();
      last().task(taskEvent(7));
      await settle();
    });
    const first = last();

    await act(async () => {
      first.fail();
      await settle();
    });

    expect(first.closed).toBe(1);
    expect(refreshAccessToken).toHaveBeenCalledTimes(1);
    expect(board().stream).toBe("reconnecting");
    expect(FakeEventSource.instances).toHaveLength(1);

    getAccessToken.mockReturnValue("token-two");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });

    expect(FakeEventSource.instances).toHaveLength(2);
    expect(last().url).toBe(
      `/api/projects/${PROJECT}/tasks/stream?token=token-two&after=7`,
    );

    view.unmount();
  });

  it("stops when the refresh answers 401", async () => {
    vi.useFakeTimers();
    refreshAccessToken.mockRejectedValue(new ApiError(401, "unauthenticated"));
    const view = renderHook(() => useTaskStream(PROJECT));
    await act(async () => {
      last().accept();
      await settle();
    });

    await act(async () => {
      last().fail();
      await settle();
      await vi.advanceTimersByTimeAsync(60_000);
    });

    expect(FakeEventSource.instances).toHaveLength(1);

    view.unmount();
  });

  it("stops when the shared refresh completed for a session that is gone", async () => {
    vi.useFakeTimers();
    // `services/auth` rejects this way when the rotation finished after the
    // user signed out: there is nothing left to reconnect to.
    refreshAccessToken.mockRejectedValue(new auth.StaleRefreshError());
    const view = renderHook(() => useTaskStream(PROJECT));
    await act(async () => {
      last().accept();
      await settle();
    });

    await act(async () => {
      last().fail();
      await settle();
      await vi.advanceTimersByTimeAsync(60_000);
    });

    expect(FakeEventSource.instances).toHaveLength(1);

    view.unmount();
  });

  it("backs off after a transient refresh failure", async () => {
    vi.useFakeTimers();
    vi.spyOn(Math, "random").mockReturnValue(0.5); // no jitter
    refreshAccessToken.mockRejectedValue(new ApiError(503, "unavailable"));
    const view = renderHook(() => useTaskStream(PROJECT));
    await act(async () => {
      last().accept();
      await settle();
    });

    await act(async () => {
      last().fail();
      await settle();
      await vi.advanceTimersByTimeAsync(999);
    });
    expect(FakeEventSource.instances).toHaveLength(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2);
    });
    expect(FakeEventSource.instances).toHaveLength(2);

    view.unmount();
  });

  it("reopens with the new token when the credentials are replaced", async () => {
    const view = renderHook(() => useTaskStream(PROJECT));
    await act(async () => {
      last().accept();
      await settle();
    });
    const handler = onCredentialsReplaced.mock.calls[0][0];

    getAccessToken.mockReturnValue("token-two");
    act(() => {
      handler("token-two");
    });

    expect(FakeEventSource.instances).toHaveLength(2);
    expect(last().url).toContain("token=token-two");
    expect(refreshAccessToken).not.toHaveBeenCalled();

    view.unmount();
  });

  it("closes on unmount and refreshes no further", async () => {
    const view = renderHook(() => useTaskStream(PROJECT));
    await act(async () => {
      last().accept();
      await settle();
    });
    const source = last();
    const loads = listTasks.mock.calls.length;

    view.unmount();

    expect(source.closed).toBe(1);

    await act(async () => {
      source.accept();
      source.task(taskEvent(9));
      source.fail();
      await settle();
    });

    expect(listTasks).toHaveBeenCalledTimes(loads);
    expect(refreshAccessToken).not.toHaveBeenCalled();
    expect(FakeEventSource.instances).toHaveLength(1);
  });

  it("does not open a stream without an access token", () => {
    getAccessToken.mockReturnValue(null);

    const view = renderHook(() => useTaskStream(PROJECT));

    expect(FakeEventSource.instances).toHaveLength(0);
    view.unmount();
  });
});
