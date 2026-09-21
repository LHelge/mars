// The one place in the frontend that talks to
// `GET /api/projects/{pid}/tasks/stream` (`SPEC.md`, "SSE: task stream", the
// stream rules of "Authentication" and "Frontend", "Task board").
//
// The connection logic is the plain `TaskStream` class — no React — so the
// open-before-load ordering, the explicit reconnect and the invalidations are
// unit-testable with a fake `EventSource`; `useTaskStream` only owns its
// lifetime. The browser's automatic `EventSource` retry is never used: every
// error closes the source, refreshes the access token and reopens with
// `?after=<lastSeq>`, which is also why `Last-Event-ID` is never relied on.
//
// The access token is read from `services/auth` at connect time and never kept
// in React state, so a refresh reconnects the stream without re-rendering the
// board.

import { useEffect } from "react";

import { queryClient } from "../queryClient";
import { ApiError } from "../services/apiClient";
import {
  getAccessToken,
  isStaleRefreshError,
  onCredentialsReplaced,
  onSignOut,
  refreshAccessToken,
} from "../services/auth";
import { taskStreamUrl } from "../services/tasks";
import type { TaskEvent, TaskEventKind } from "../types";
import { backoffDelay } from "../utils/backoff";
import { taskKeys } from "./queryKeys";
import type { TaskStreamStatus } from "./taskStore";
import { useTaskStore } from "./taskStore";

/** The parts of `EventSource` this module uses, so tests can inject a fake. */
export interface EventSourceLike {
  close(): void;
  onopen: ((ev: Event) => void) | null;
  onerror: ((ev: Event) => void) | null;
  addEventListener(
    type: "task",
    listener: (ev: MessageEvent<string>) => void,
  ): void;
}

export type EventSourceFactory = (url: string) => EventSourceLike;

/**
 * Kinds that change what `GET /sessions/{id}/tasks` returns for some session:
 * a hand-over, a hold taken or given back, or a comment shown in the panel.
 */
const SESSION_TASK_KINDS: ReadonlySet<TaskEventKind> = new Set<TaskEventKind>([
  "commented",
  "state_changed",
  "claimed",
  "released",
]);

/** `queryKeys.sessions.tasks(id)` is `["sessions", <id>, "tasks"]`. */
function isSessionTasksKey(key: readonly unknown[]): boolean {
  return key.length === 3 && key[0] === "sessions" && key[2] === "tasks";
}

function isUnauthorized(error: unknown): boolean {
  return error instanceof ApiError && error.status === 401;
}

const browserEventSource: EventSourceFactory = (url) => new EventSource(url);

export class TaskStream {
  readonly projectId: string;

  private readonly factory: EventSourceFactory;
  private source: EventSourceLike | null = null;
  private disposed = false;
  private attempt = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  /**
   * Our own reconnecting refresh is in flight; a replacement that lands while
   * it runs is ignored here, because that refresh resolves with the current
   * credentials and reopens once with them.
   */
  private refreshing = false;
  private readonly unsubscribes: (() => void)[] = [];

  constructor(projectId: string, factory: EventSourceFactory = browserEventSource) {
    this.projectId = projectId;
    this.factory = factory;
  }

  private get store() {
    return useTaskStore.getState();
  }

  /**
   * Binds the board to this project and opens the stream. No REST load is
   * started here: the first refresh is the stream's `open` handler, so the
   * snapshot can never be older than the cursor the stream resumes from
   * (`SPEC.md`, "SSE: task stream").
   */
  start(): void {
    this.unsubscribes.push(
      onCredentialsReplaced(() => {
        this.handleCredentialsReplaced();
      }),
      onSignOut(() => {
        this.handleSignOut();
      }),
    );
    this.store.bindProject(this.projectId);
    this.store.setStream("connecting");
    this.connect();
  }

  private connect(): void {
    if (this.disposed || this.source !== null) return;
    const token = getAccessToken();
    if (token === null) return; // Signed out; the route guard sends us away.

    // Captured before the source exists, so a handler that arrives after a
    // project change or a reconnect cannot write to the current view.
    const view = this.store.viewGeneration;
    const source = this.factory(
      taskStreamUrl(this.projectId, token, this.store.lastSeq),
    );
    this.source = source;
    source.onopen = () => {
      if (this.disposed || this.store.viewGeneration !== view) return;
      this.attempt = 0;
      this.store.setStream("live");
      // The initial load on the first open, the catch-up refresh afterwards.
      void this.store.refresh();
    };
    source.addEventListener("task", (ev) => {
      if (this.disposed || this.store.viewGeneration !== view) return;
      this.handleEvent(ev.data);
    });
    source.onerror = () => {
      this.handleError();
    };
  }

  private handleEvent(data: string): void {
    let event: TaskEvent;
    try {
      event = JSON.parse(data) as TaskEvent;
    } catch {
      console.warn("task stream: malformed event ignored");
      return;
    }
    // A replayed or duplicate `seq` changes nothing and owes no refresh.
    if (!this.store.noteEvent(event)) return;
    const taskId = event.task_id;
    if (taskId === null || taskId === undefined) return;
    // `taskKeys.all` is the prefix of every task detail key, so one
    // invalidation covers the board snapshot and any open drawer.
    void queryClient.invalidateQueries({
      queryKey: taskKeys.all(this.projectId),
    });
    if (SESSION_TASK_KINDS.has(event.kind)) {
      // Best effort, and only the per-session task lists: matching the whole
      // `["sessions"]` prefix would refetch every session query in the app.
      void queryClient.invalidateQueries({
        predicate: (query) => isSessionTasksKey(query.queryKey),
      });
    }
  }

  /** Drops our handlers before closing, so this close never reconnects. */
  private teardown(): void {
    const source = this.source;
    this.source = null;
    if (source === null) return;
    source.onopen = null;
    source.onerror = null;
    try {
      source.close();
    } catch {
      // Already closed; nothing left to do.
    }
  }

  /**
   * `onerror` fires both for a dropped connection and for a refused one that
   * never opened (an expired token answers 401 while still `CONNECTING`), so
   * both take the same path: close, refresh, reopen at the last cursor.
   */
  private handleError(): void {
    if (this.disposed) return;
    this.teardown();
    this.store.setStream("reconnecting");
    // Anything still in flight belongs to the connection that just died.
    this.store.bumpView();
    void this.refreshThenReconnect();
  }

  private async refreshThenReconnect(): Promise<void> {
    this.refreshing = true;
    try {
      // One rotation for the whole browser: `services/auth` shares this with
      // the HTTP client and the session socket, and answers with whatever
      // credentials are current if a newer login replaced ours meanwhile.
      await refreshAccessToken();
    } catch (error) {
      // A 401 already cleared local authentication and routed to login, and a
      // stale completion belongs to a session that is gone; only transient
      // failures are worth retrying.
      if (!isUnauthorized(error) && !isStaleRefreshError(error)) {
        this.scheduleRetry();
      }
      return;
    } finally {
      this.refreshing = false;
    }
    this.scheduleRetry();
  }

  private scheduleRetry(): void {
    if (this.disposed || this.retryTimer !== null) return;
    const delay = backoffDelay(this.attempt);
    this.attempt += 1;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      this.connect();
    }, delay);
  }

  private cancelRetry(): void {
    if (this.retryTimer === null) return;
    clearTimeout(this.retryTimer);
    this.retryTimer = null;
  }

  /**
   * The browser's credentials were really replaced — a self-service password
   * change, or a login as somebody else — so this stream's authorization is
   * gone and it reopens with the new token. An ordinary refresh rotation does
   * not come through here at all (`services/auth`, `InstallReason`): an open
   * stream is not closed because its token expired (`SPEC.md`,
   * "Authentication"), and reopening would re-read the whole board snapshot.
   */
  private handleCredentialsReplaced(): void {
    if (this.disposed || this.refreshing) return;
    this.cancelRetry();
    this.attempt = 0;
    this.teardown();
    this.store.setStream("reconnecting");
    this.store.bumpView();
    this.connect();
  }

  private handleSignOut(): void {
    this.cancelRetry();
    this.teardown();
  }

  /** Closes the stream, cancels pending backoff and retires the view. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    for (const unsubscribe of this.unsubscribes.splice(0)) unsubscribe();
    this.cancelRetry();
    this.teardown();
    // A read still in flight can no longer update the store.
    this.store.bumpView();
  }
}

/**
 * Binds the task board store to `projectId` for as long as the component is
 * mounted and keeps its stream open. Returns the stream status, the only part
 * of the connection a view renders.
 */
export function useTaskStream(
  projectId: string,
  factory?: EventSourceFactory,
): TaskStreamStatus {
  const status = useTaskStore((state) => state.stream);

  useEffect(() => {
    const stream = new TaskStream(projectId, factory);
    stream.start();
    return () => {
      stream.dispose();
    };
  }, [projectId, factory]);

  return status;
}
