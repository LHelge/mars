// The one place in the frontend that talks to
// `GET /api/projects/{pid}/tasks/stream` (`SPEC.md`, "SSE: task stream", the
// stream rules of "Authentication" and "Frontend", "Task board").
//
// The connection logic is the plain `TaskStream` class — no React — so the
// open-before-load ordering, the explicit reconnect and the invalidations are
// unit-testable with a fake `EventSource`; `useTaskStream` only owns its
// lifetime. The browser's automatic `EventSource` retry is never used: every
// error closes the source and reopens it with `?after=<lastSeq>` — the access
// token rotated on the first error of a run of failures and on no other, and
// the attempts bounded and ended visibly (`handleError`) — which is also why
// `Last-Event-ID` is never relied on. A
// board that has received no event yet has no such cursor and opens at
// `?after=latest` instead (`services/tasks`, `taskStreamUrl`), so a project's
// whole history is never replayed into a client that is about to read a REST
// snapshot anyway.
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
import { errorMessage, isNotFound } from "../services/errorMessage";
import { getProject } from "../services/projects";
import { taskStreamUrl } from "../services/tasks";
import type { TaskEvent } from "../types";
import { backoffDelay } from "../utils/backoff";
import { coalesce } from "../utils/coalesce";
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

function isUnauthorized(error: unknown): boolean {
  return error instanceof ApiError && error.status === 401;
}

function isForbidden(error: unknown): boolean {
  return error instanceof ApiError && error.status === 403;
}

/**
 * How many errors that never reached `open` are made before the project is
 * read over REST. `EventSource` never hands JavaScript the status behind a
 * refused connection, so a stream that cannot open has to ask the endpoint
 * that answers in words (the same rule as `session/useSessionSocket`).
 */
const ATTEMPTS_BEFORE_CHECK = 5;

/** How many such attempts end it, the check having found nothing wrong. */
const ATTEMPTS_BEFORE_OFFLINE = 10;

/** What the reader is told when the project is no longer there. */
const GONE = "This project no longer exists.";
/** ... when it is there but no longer theirs. */
const FORBIDDEN = "You no longer have access to this project.";
/** ... when their authentication itself is gone. */
const SIGNED_OUT = "Your sign-in has ended. Sign in again to reconnect.";
/** ... when nothing answered at all for as long as we kept trying. */
const UNREACHABLE = "Could not reconnect to the task stream.";

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
  /** The current source reached `open`; reset by every `connect`. */
  private opened = false;
  /** Errors since the last connection that opened; the loop's bound. */
  private failedOpens = 0;
  /** The REST check has already been made for this run of failures. */
  private checked = false;
  /** A REST check is in flight; a second error must not start another. */
  private checking = false;
  private readonly unsubscribes: (() => void)[] = [];

  /**
   * One `invalidateQueries` per tick, however many events arrived in it
   * (`SPEC.md`, "Frontend", "Board refresh ordering"). Each call is a scan of
   * the query cache and a refetch of every open task detail, and a project
   * with agents working in it emits events faster than that is worth doing.
   */
  private readonly invalidate = coalesce(() => {
    // `taskKeys.all` is the prefix of every task detail key, so one
    // invalidation covers the board snapshot and any open drawer.
    void queryClient.invalidateQueries({
      queryKey: taskKeys.all(this.projectId),
    });
  });

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
    this.store.setStreamRetry(() => {
      this.reconnect();
    });
    this.connect();
  }

  /**
   * Start again from nothing: the board's recovery action out of `offline`,
   * and the path a credential replacement takes. Every count the last run
   * left behind is dropped.
   */
  reconnect(): void {
    if (this.disposed) return;
    this.cancelRetry();
    this.attempt = 0;
    this.failedOpens = 0;
    this.checked = false;
    this.teardown();
    this.store.setStream("reconnecting");
    this.store.bumpView();
    this.connect();
  }

  private connect(): void {
    if (this.disposed || this.source !== null) return;
    const token = getAccessToken();
    if (token === null) {
      // Signed out; the route guard sends us away. Say so rather than leave a
      // status claiming an attempt that will never be made.
      this.store.setStream("offline", SIGNED_OUT);
      return;
    }
    this.opened = false;

    // Captured before the source exists, so a handler that arrives after a
    // project change or a reconnect cannot write to the current view.
    const view = this.store.viewGeneration;
    const source = this.factory(
      taskStreamUrl(this.projectId, token, this.store.lastSeq),
    );
    this.source = source;
    source.onopen = () => {
      if (this.disposed || this.store.viewGeneration !== view) return;
      this.opened = true;
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
    // Every kind, including the `states_changed` that carries no `task_id`: a
    // renamed state is exactly what an open drawer is showing the old name of
    // (`SPEC.md`, "Frontend", "Board refresh ordering").
    this.invalidate.run();
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
   * never opened (an expired token answers 401 while still `CONNECTING`), and
   * `EventSource` never says which, so the token is rotated on the first
   * error of a run and never again: a stream that cannot open — the project
   * was deleted, say — would otherwise spend the single-use refresh cookie on
   * every cycle for as long as the tab was left open, while telling the
   * reader a reconnect was underway.
   *
   * The attempts that never open are counted, and after
   * `ATTEMPTS_BEFORE_CHECK` the project is read over REST, which answers in
   * words where the stream cannot.
   */
  private handleError(): void {
    if (this.disposed) return;
    this.teardown();
    // Anything still in flight belongs to the connection that just died.
    this.store.bumpView();

    if (this.opened) {
      this.failedOpens = 0;
      this.checked = false;
      this.attempt = 0;
    } else {
      this.failedOpens += 1;
    }

    if (this.failedOpens <= 1) {
      // The first error of this run: an expired token is a real reading of it,
      // and the rotation is what `SPEC.md`, "Authentication" asks for.
      this.store.setStream("reconnecting");
      void this.refreshThenReconnect();
      return;
    }
    this.retryOrCheck();
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
      if (isUnauthorized(error) || isStaleRefreshError(error)) {
        this.giveUp(SIGNED_OUT);
        return;
      }
      this.retryOrCheck();
      return;
    } finally {
      this.refreshing = false;
    }
    this.retryOrCheck();
  }

  /**
   * The bound on reconnection: back off while attempts are cheap, ask REST
   * once they are not, and stop with something the reader can act on rather
   * than reopening at the 30 s cap until the tab is closed.
   */
  private retryOrCheck(): void {
    if (this.disposed) return;
    if (this.failedOpens >= ATTEMPTS_BEFORE_OFFLINE) {
      this.giveUp(UNREACHABLE);
      return;
    }
    this.store.setStream("reconnecting");
    if (this.failedOpens >= ATTEMPTS_BEFORE_CHECK && !this.checked) {
      void this.checkProject();
      return;
    }
    this.scheduleRetry();
  }

  /** Reads the project once over REST to decide what the errors mean. */
  private async checkProject(): Promise<void> {
    if (this.checking || this.disposed) return;
    this.checking = true;
    this.checked = true;
    try {
      await getProject(this.projectId);
    } catch (error) {
      if (this.disposed) return;
      if (isNotFound(error)) {
        this.giveUp(GONE);
        return;
      }
      if (isForbidden(error)) {
        this.giveUp(FORBIDDEN);
        return;
      }
      if (isUnauthorized(error)) {
        // `apiClient` has already rotated once and failed; `services/auth`
        // has cleared this browser's authentication and is routing to login.
        this.giveUp(SIGNED_OUT);
        return;
      }
      console.warn("task stream project check failed:", errorMessage(error));
      this.scheduleRetry();
      return;
    } finally {
      this.checking = false;
    }
    if (this.disposed) return;
    // The project is there and still ours: waiting is worth something, under
    // the same bound as any other attempt.
    this.scheduleRetry();
  }

  /**
   * The end of the line: no timer, no rotation, no pretence that a reconnect
   * is underway — a sentence saying why and the board's Reconnect action.
   */
  private giveUp(message: string): void {
    this.cancelRetry();
    this.teardown();
    this.store.setStream("offline", message);
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
    this.reconnect();
  }

  private handleSignOut(): void {
    this.invalidate.cancel();
    this.cancelRetry();
    this.teardown();
    this.store.setStream("offline", SIGNED_OUT);
  }

  /** Closes the stream, cancels pending backoff and retires the view. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    for (const unsubscribe of this.unsubscribes.splice(0)) unsubscribe();
    this.invalidate.cancel();
    this.cancelRetry();
    this.teardown();
    this.store.setStreamRetry(null);
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
