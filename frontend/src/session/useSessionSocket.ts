// The one place in the frontend that talks to `/ws/sessions/{id}`
// (`SPEC.md`, "WebSocket: session stream", "Authentication" stream rules and
// "Frontend", "Session state").
//
// The connection logic is the plain `SessionSocket` class — no React — so the
// reconnect, replay and terminal behaviour is unit-testable with a fake socket;
// `useSessionSocket` only owns its lifetime and hands out stable callbacks.
// `SessionView` mounts it once per page and passes the API down.

import { useCallback, useEffect, useMemo, useRef } from "react";
import type { StoreApi } from "zustand";

import { ApiError } from "../services/apiClient";
import { errorMessage } from "../services/errorMessage";
import {
  getAccessToken,
  isStaleRefreshError,
  onCredentialsReplaced,
  onSignOut,
  refreshAccessToken,
} from "../services/auth";
import { listEvents, sendInput, stopSession } from "../services/sessions";
import type { ClientMessage, ServerMessage, SessionInput } from "../types";
import { backoffDelay } from "../utils/backoff";
import type { ConnectionStatus, SessionStore } from "./sessionStore";
import { getSessionStore, useSessionStore } from "./sessionStore";
import { buildSessionSocketUrl } from "./socketUrl";

/** The history page size; the endpoint caps `limit` at 500. */
const PAGE_SIZE = 200;
/** The close code the orchestrator uses after `authentication required`. */
const AUTH_CLOSE_CODE = 1008;
/** A second auth close inside this window means refreshing did not help. */
const AUTH_RETRY_WINDOW_MS = 5000;
const AUTH_ERROR = "authentication required";
const OPEN = 1;

/** Delivered to terminal subscribers: PTY bytes, or the exit notice. */
export type TerminalFrame = Uint8Array | { exit_code: number };

export type TerminalListener = (frame: TerminalFrame) => void;

/** The parts of `WebSocket` this module uses, so tests can inject a fake. */
export interface SocketLike {
  binaryType: BinaryType;
  readonly readyState: number;
  send(data: string | ArrayBuffer): void;
  close(code?: number, reason?: string): void;
  onopen: ((ev: Event) => void) | null;
  onmessage: ((ev: MessageEvent) => void) | null;
  onclose: ((ev: CloseEvent) => void) | null;
  onerror: ((ev: Event) => void) | null;
}

export type SocketFactory = (url: string) => SocketLike;

const browserSocket: SocketFactory = (url) => new WebSocket(url);

export interface TerminalApi {
  open: (cols: number, rows: number) => void;
  resize: (cols: number, rows: number) => void;
  write: (bytes: Uint8Array) => void;
  close: () => void;
  subscribe: (listener: TerminalListener) => () => void;
}

export interface SessionSocketApi {
  status: ConnectionStatus;
  /** Returns the generated `client_id` the `user_message` echoes back. */
  send: (input: SessionInput) => string;
  stop: () => void;
  /** Resolves `false` when the request failed; see `SessionSocket.loadOlder`. */
  loadOlder: () => Promise<boolean>;
  terminal: TerminalApi;
}

function reason(error: unknown): string {
  return errorMessage(error);
}

function isUnauthorized(error: unknown): boolean {
  return error instanceof ApiError && error.status === 401;
}

export class SessionSocket {
  readonly sessionId: string;

  private readonly store: StoreApi<SessionStore>;
  private readonly factory: SocketFactory;

  private socket: SocketLike | null = null;
  private disposed = false;
  /** The closed connection had reached `open`: reconnect without backoff. */
  private wasOpen = false;
  private attempt = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  /**
   * Our own reconnecting refresh is in flight; a replacement that lands while
   * it runs is ignored here, because that refresh resolves with the current
   * credentials and connects once with them.
   */
  private refreshing = false;
  private sawAuthError = false;
  private lastAuthCloseAt: number | null = null;
  /** The older-history request in flight, which later callers join. */
  private loadingOlder: Promise<boolean> | null = null;
  private terminalRunning = false;
  private readonly terminalListeners = new Set<TerminalListener>();
  private readonly unsubscribes: (() => void)[] = [];

  constructor(sessionId: string, factory: SocketFactory = browserSocket) {
    this.sessionId = sessionId;
    this.store = getSessionStore(sessionId);
    this.factory = factory;
  }

  get isDisposed(): boolean {
    return this.disposed;
  }

  /**
   * Loads the newest history page when this session has none yet, then opens
   * the socket at `after = lastSeq`.
   *
   * The store is per session id (`getSessionStore`), so returning to a session
   * resumes from the transcript already folded: nothing is reset and REST is
   * skipped whenever `lastSeq > 0`.
   */
  async start(): Promise<void> {
    this.unsubscribes.push(
      onCredentialsReplaced(() => {
        this.onCredentialsReplaced();
      }),
      onSignOut(() => {
        this.onSignOut();
      }),
    );
    this.store.getState().setStatus("connecting");
    if (this.store.getState().lastSeq === 0) {
      try {
        const page = await listEvents(this.sessionId, { limit: PAGE_SIZE });
        if (this.disposed) return;
        this.store.getState().prependHistory(page.events, page.has_more);
      } catch (error) {
        // The socket replays from `after = 0` anyway, so a failed page is not
        // fatal: log it and connect.
        console.warn("session history failed to load:", reason(error));
        if (this.disposed) return;
      }
    }
    this.connect();
  }

  private connect(): void {
    if (this.disposed || this.socket !== null) return;
    const token = getAccessToken();
    if (token === null) return; // Signed out; `onSignOut` already stopped us.

    const after = this.store.getState().lastSeq;
    const socket = this.factory(buildSessionSocketUrl(this.sessionId, after, token));
    socket.binaryType = "arraybuffer";
    this.socket = socket;
    this.wasOpen = false;
    socket.onopen = () => {
      this.wasOpen = true;
      this.attempt = 0;
      this.store.getState().setStatus("live");
    };
    socket.onmessage = (ev) => {
      this.handleFrame(ev.data);
    };
    socket.onerror = () => {
      // A close always follows; reconnection is decided there.
    };
    socket.onclose = (ev) => {
      this.handleClose(ev.code);
    };
  }

  /** Drops our handlers before closing, so this close never reconnects. */
  private teardown(code: number): void {
    const socket = this.socket;
    this.socket = null;
    this.terminalRunning = false;
    if (socket === null) return;
    socket.onopen = null;
    socket.onmessage = null;
    socket.onclose = null;
    socket.onerror = null;
    try {
      socket.close(code);
    } catch {
      // Already closing; nothing left to do.
    }
  }

  private handleFrame(data: unknown): void {
    if (data instanceof ArrayBuffer) {
      this.emitTerminal(new Uint8Array(data));
      return;
    }
    if (ArrayBuffer.isView(data)) {
      this.emitTerminal(
        new Uint8Array(data.buffer, data.byteOffset, data.byteLength),
      );
      return;
    }
    if (typeof data !== "string") {
      console.warn("session socket: unsupported frame type");
      return;
    }
    let message: ServerMessage;
    try {
      message = JSON.parse(data) as ServerMessage;
    } catch {
      console.warn("session socket: malformed frame ignored");
      return;
    }
    this.dispatch(message);
  }

  private dispatch(message: ServerMessage): void {
    const store = this.store.getState();
    switch (message.type) {
      case "event":
        store.applyEvent(message.event);
        return;
      case "session":
        store.setSession(message.session);
        return;
      case "input_accepted":
        store.inputAccepted(message.client_id, message.seq);
        return;
      case "input_rejected":
        store.inputRejected(message.client_id, message.reason);
        return;
      case "terminal_closed":
        this.terminalRunning = false;
        this.emitTerminal({ exit_code: message.exit_code });
        return;
      case "error":
        if (message.message === AUTH_ERROR) {
          // The close with 1008 follows; it does the refresh and reconnect.
          this.sawAuthError = true;
          return;
        }
        store.addSystemMessage(message.message, "error");
        return;
    }
  }

  private handleClose(code: number): void {
    this.socket = null;
    this.terminalRunning = false;
    if (this.disposed) return;
    this.store.getState().setStatus("reconnecting");

    if (this.sawAuthError || code === AUTH_CLOSE_CODE) {
      this.sawAuthError = false;
      const now = Date.now();
      if (
        this.lastAuthCloseAt !== null &&
        now - this.lastAuthCloseAt < AUTH_RETRY_WINDOW_MS
      ) {
        // A freshly refreshed token was refused again: stop and let the
        // foundation route to login.
        this.lastAuthCloseAt = null;
        return;
      }
      this.lastAuthCloseAt = now;
    }

    void this.refreshThenReconnect();
  }

  private async refreshThenReconnect(): Promise<void> {
    this.refreshing = true;
    try {
      // One rotation for the whole browser: `services/auth` shares this with
      // the HTTP client and the task stream, and answers with whatever
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
    if (this.disposed) return;
    if (this.wasOpen) {
      this.attempt = 0;
      this.connect();
    } else {
      // The last attempt never opened: back off rather than spin.
      this.scheduleRetry();
    }
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
   * change, or a login as somebody else — so this socket's authorization is
   * gone and it reopens with the new token. An ordinary refresh rotation does
   * not come through here at all (`services/auth`, `InstallReason`): an open
   * stream is not closed because its token expired (`SPEC.md`,
   * "Authentication"), and closing it would dispose the exec PTY behind the
   * terminal.
   */
  private onCredentialsReplaced(): void {
    if (this.disposed || this.refreshing) return;
    this.cancelRetry();
    this.attempt = 0;
    this.teardown(1000);
    this.store.getState().setStatus("reconnecting");
    this.connect();
  }

  private onSignOut(): void {
    this.cancelRetry();
    this.teardown(1000);
    this.store.getState().setStatus("reconnecting");
  }

  private sendJson(message: ClientMessage): boolean {
    const socket = this.socket;
    if (socket === null || socket.readyState !== OPEN) return false;
    socket.send(JSON.stringify(message));
    return true;
  }

  send(input: SessionInput): string {
    const clientId = globalThis.crypto.randomUUID();
    this.store.getState().addOptimisticUser(clientId, input);
    if (!this.sendJson({ type: "input", client_id: clientId, input })) {
      // Closed or reconnecting: the REST equivalent accepts it with 202, and
      // carries the same `client_id`, so the `user_message` that follows
      // replaces the optimistic message instead of doubling it.
      void sendInput(this.sessionId, input, clientId).catch((error: unknown) => {
        this.store.getState().inputRejected(clientId, reason(error));
      });
    }
    return clientId;
  }

  stop(): void {
    if (this.sendJson({ type: "stop" })) return;
    void stopSession(this.sessionId).catch((error: unknown) => {
      this.store.getState().addSystemMessage(`Stop failed: ${reason(error)}`);
    });
  }

  /**
   * Fetches the page before `oldestSeq`. Resolves `true` when there is nothing
   * more to do — a page landed, or there was none to ask for — and `false` when
   * the request failed, which also leaves `historyStatus` at `error` for the
   * transcript to offer a retry on. Simultaneous callers — a scroll gesture and
   * the layout effect that fills a short transcript — share the one request
   * rather than racing two pages of the same cursor.
   */
  loadOlder(): Promise<boolean> {
    const running = this.loadingOlder;
    if (running !== null) return running;
    const request = this.fetchOlder();
    this.loadingOlder = request;
    void request.finally(() => {
      if (this.loadingOlder === request) this.loadingOlder = null;
    });
    return request;
  }

  private async fetchOlder(): Promise<boolean> {
    const state = this.store.getState();
    if (!state.hasMore) return true;
    const before = state.oldestSeq;
    if (before === null) return true;
    if (before <= 1) {
      // `seq` starts at 1, so nothing can precede it.
      state.prependHistory([], false);
      return true;
    }
    state.setHistoryStatus("loading");
    try {
      const page = await listEvents(this.sessionId, { before, limit: PAGE_SIZE });
      this.store.getState().prependHistory(page.events, page.has_more);
      this.store.getState().setHistoryStatus("idle");
      return true;
    } catch (error) {
      // The transcript keeps what it holds; only the status changes, so the
      // reader sees why and can ask again.
      console.warn("older session history failed to load:", reason(error));
      this.store.getState().setHistoryStatus("error", reason(error));
      return false;
    }
  }

  private emitTerminal(frame: TerminalFrame): void {
    for (const listener of [...this.terminalListeners]) listener(frame);
  }

  readonly terminal: TerminalApi = {
    open: (cols, rows) => {
      if (this.store.getState().session?.state !== "running") {
        // The same answer the orchestrator gives a terminal it cannot honour.
        this.emitTerminal({ exit_code: -1 });
        return;
      }
      if (this.sendJson({ type: "terminal_open", cols, rows })) {
        this.terminalRunning = true;
      }
    },
    resize: (cols, rows) => {
      if (!this.terminalRunning) return;
      this.sendJson({ type: "terminal_resize", cols, rows });
    },
    write: (bytes) => {
      const socket = this.socket;
      if (!this.terminalRunning || socket === null || socket.readyState !== OPEN) {
        return;
      }
      // A copy, so the frame never carries more of the caller's buffer than
      // the bytes it was given.
      const frame = new Uint8Array(bytes.byteLength);
      frame.set(bytes);
      socket.send(frame.buffer);
    },
    close: () => {
      if (!this.terminalRunning) return;
      this.terminalRunning = false;
      this.sendJson({ type: "terminal_close" });
    },
    subscribe: (listener) => {
      this.terminalListeners.add(listener);
      return () => {
        this.terminalListeners.delete(listener);
      };
    },
  };

  /** Closes the terminal, then the socket, and cancels pending backoff. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    for (const unsubscribe of this.unsubscribes.splice(0)) unsubscribe();
    this.cancelRetry();
    this.terminal.close();
    this.teardown(1000);
    this.terminalListeners.clear();
  }
}

export function useSessionSocket(
  sessionId: string,
  factory?: SocketFactory,
): SessionSocketApi {
  const ref = useRef<SessionSocket | null>(null);
  const status = useSessionStore(sessionId, (state) => state.status);

  const socketFor = useCallback(
    (id: string): SessionSocket => {
      const current = ref.current;
      if (current === null || current.sessionId !== id || current.isDisposed) {
        const next = new SessionSocket(id, factory);
        ref.current = next;
        return next;
      }
      return current;
    },
    [factory],
  );

  useEffect(() => {
    const socket = socketFor(sessionId);
    void socket.start();
    return () => {
      socket.dispose();
    };
  }, [sessionId, socketFor]);

  const send = useCallback(
    (input: SessionInput) => socketFor(sessionId).send(input),
    [sessionId, socketFor],
  );
  const stop = useCallback(() => {
    socketFor(sessionId).stop();
  }, [sessionId, socketFor]);
  const loadOlder = useCallback(
    () => socketFor(sessionId).loadOlder(),
    [sessionId, socketFor],
  );
  // One facade per session id, so `TerminalView` can depend on its identity.
  const terminal = useMemo<TerminalApi>(
    () => ({
      open: (cols, rows) => {
        socketFor(sessionId).terminal.open(cols, rows);
      },
      resize: (cols, rows) => {
        socketFor(sessionId).terminal.resize(cols, rows);
      },
      write: (bytes) => {
        socketFor(sessionId).terminal.write(bytes);
      },
      close: () => {
        socketFor(sessionId).terminal.close();
      },
      subscribe: (listener) => socketFor(sessionId).terminal.subscribe(listener),
    }),
    [sessionId, socketFor],
  );

  return { status, send, stop, loadOlder, terminal };
}
