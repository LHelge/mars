// The one place in the frontend that talks to `/ws/sessions/{id}`
// (`SPEC.md`, "WebSocket: session stream", "Authentication" stream rules and
// "Frontend", "Session state").
//
// The connection logic is the plain `SessionSocket` class — no React — so the
// reconnect, replay and terminal behaviour is unit-testable with a fake socket;
// `useSessionSocket` only owns its lifetime and hands out stable callbacks.
// `SessionPage` mounts it once per page and hands the API to `SessionView`,
// which publishes it on `SessionSocketContext`.

import { useCallback, useEffect, useMemo, useRef } from "react";
import type { StoreApi } from "zustand";

import { ApiError } from "../services/apiClient";
import { errorMessage, isNotFound } from "../services/errorMessage";
import {
  getAccessToken,
  isStaleRefreshError,
  onCredentialsReplaced,
  onSignOut,
  refreshAccessToken,
} from "../services/auth";
import { getSession, listEvents, sendInput, stopSession } from "../services/sessions";
import type { ClientMessage, ServerMessage, SessionInput } from "../types";
import { backoffDelay } from "../utils/backoff";
import type { ConnectionStatus, SessionStore } from "./sessionStore";
import {
  getSessionStore,
  peekSessionStore,
  releaseSessionStore,
  retainSessionStore,
  sessionStoreGeneration,
  useSessionStore,
} from "./sessionStore";
import { HISTORY_PAGE_SIZE } from "./historyPageSize";
import { parseServerMessage } from "./serverMessage";
import { buildSessionSocketUrl } from "./socketUrl";

/** The close code the orchestrator uses after `authentication required`. */
const AUTH_CLOSE_CODE = 1008;
/** A second auth close inside this window means refreshing did not help. */
const AUTH_RETRY_WINDOW_MS = 5000;
const AUTH_ERROR = "authentication required";
const OPEN = 1;

/**
 * How many attempts that never reached `open` are made before the session is
 * read over REST. A refused upgrade is a close with no status — the browser
 * never hands JavaScript the 404 or 403 behind it — so a socket that cannot
 * open has to ask the one endpoint that answers in words.
 */
const ATTEMPTS_BEFORE_CHECK = 5;

/**
 * How many such attempts end it. The check only says the session is still
 * there and still ours; when that many attempts on top of it still never
 * open, the reason is not one this client can wait out.
 */
const ATTEMPTS_BEFORE_OFFLINE = 10;

/** What the reader is told when the session is no longer there. */
const GONE = "This session no longer exists.";
/** ... when it is there but no longer theirs. */
const FORBIDDEN = "You no longer have access to this session.";
/** ... when their authentication itself is gone. */
const SIGNED_OUT = "Your sign-in has ended. Sign in again to reconnect.";
/**
 * ... when the server refuses the stream although the session reads fine over
 * REST with the same credentials: the refusal is about this stream, not about
 * them, and refreshing the token has already been tried and changed nothing.
 */
const REFUSED = "The server refused this session's live stream.";
/** ... when nothing answered at all for as long as we kept trying. */
const UNREACHABLE = "Could not reconnect to this session.";
/**
 * ... and what a message the connection carried away unacknowledged says about
 * itself. `readyState === OPEN` is true of a socket that died while the tab
 * slept, so a send can go out and never be answered; ADR 0020 gives no delivery
 * guarantee but does have the user resend by hand, which they can only do if
 * the message stops claiming to be on its way.
 */
const UNACKNOWLEDGED = "the connection closed before this was acknowledged";

/** What a REST check that answered decides, when it answers "still yours". */
type Alive = "retry" | "stop";

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
  /** Why the connection gave up; `null` unless `status` is `offline`. */
  error: string | null;
  /** Returns the generated `client_id` the `user_message` echoes back. */
  send: (input: SessionInput) => string;
  stop: () => void;
  /** The reader's recovery action out of `offline`: start again from scratch. */
  reconnect: () => void;
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

function isForbidden(error: unknown): boolean {
  return error instanceof ApiError && error.status === 403;
}

/** Silences one socket for good: no handler of ours points at it again. */
function detach(socket: SocketLike): void {
  socket.onopen = null;
  socket.onmessage = null;
  socket.onclose = null;
  socket.onerror = null;
}

export class SessionSocket {
  readonly sessionId: string;

  private readonly factory: SocketFactory;

  /**
   * Always the store currently bound to this id, never a captured one: a
   * clear can replace what the registry holds, and writing into the object
   * that was there before would be writing into nothing.
   */
  private get store(): StoreApi<SessionStore> {
    return getSessionStore(this.sessionId);
  }

  private socket: SocketLike | null = null;
  private disposed = false;
  /** This connection holds the store open (`retainSessionStore`). */
  private retained = false;
  /**
   * The store generation this connection belongs to. A clear moves it on, and
   * everything this object would write afterwards belongs to the login that
   * clear ended (`sessionStore`, the registry).
   */
  private generation = sessionStoreGeneration();
  /**
   * Signed out: nothing more is attempted and nothing more is written, until
   * a `reconnect()` adopts whatever credentials the browser holds now.
   */
  private invalidated = false;
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
  /** Closes since the last one that had reached `open`; the loop's bound. */
  private failedOpens = 0;
  /**
   * `failedOpens` as it stood at the last REST read of this run, `0` for none.
   * The read happens twice at most: once at the first close that never
   * opened, where it is what settles an expired token, and once at
   * `ATTEMPTS_BEFORE_CHECK`, where it is what tells a session that is gone
   * from one that is merely unreachable.
   */
  private checkedAt = 0;
  /** A REST check is in flight; a second close must not start another. */
  private checking = false;
  /** The older-history request in flight, which later callers join. */
  private loadingOlder: Promise<boolean> | null = null;
  private terminalRunning = false;
  private readonly terminalListeners = new Set<TerminalListener>();
  private readonly unsubscribes: (() => void)[] = [];

  constructor(sessionId: string, factory: SocketFactory = browserSocket) {
    this.sessionId = sessionId;
    this.factory = factory;
  }

  get isDisposed(): boolean {
    return this.disposed;
  }

  /**
   * Nothing this object does may touch the store any more: it is disposed, it
   * is signed out, or the store it was folding into belongs to a login that
   * has ended. Every path that resumes after an `await` asks this before it
   * writes, so a history page or an input rejection that lands late is
   * dropped rather than folded into whoever holds the tab now.
   */
  private get dead(): boolean {
    return (
      this.disposed ||
      this.invalidated ||
      this.generation !== sessionStoreGeneration()
    );
  }

  /**
   * Loads the newest history page when this session has none yet, then opens
   * the socket at `after = lastSeq`.
   *
   * The store is per session id and outlives the page while the registry
   * keeps it (`sessionStore`, the registry), so returning to a session resumes
   * from the transcript already folded: nothing is reset and REST is skipped
   * whenever `lastSeq > 0`. This connection is that store's owner for as long
   * as it lives, which is what makes it un-evictable.
   */
  async start(): Promise<void> {
    if (this.disposed) return;
    this.generation = sessionStoreGeneration();
    if (!this.retained) {
      this.retained = true;
      retainSessionStore(this.sessionId);
    }
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
        const page = await listEvents(this.sessionId, { limit: HISTORY_PAGE_SIZE });
        if (this.dead) return;
        this.store.getState().prependHistory(page.events, page.has_more);
      } catch (error) {
        // The socket replays from `after = 0` anyway, so a failed page is not
        // fatal: log it and connect.
        console.warn("session history failed to load:", reason(error));
        if (this.dead) return;
      }
    }
    this.connect();
  }

  private connect(): void {
    if (this.dead || this.socket !== null) return;
    const token = getAccessToken();
    if (token === null) {
      // Signed out; `onSignOut` already stopped us. Say so rather than leave
      // a status claiming an attempt that will never be made.
      this.store.getState().setStatus("offline", SIGNED_OUT);
      return;
    }

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
      // Detached first: a socket that has closed is nobody's stream any more,
      // and a late frame from it must not reach the store behind the
      // connection that replaces it.
      detach(socket);
      this.handleClose(ev.code);
    };
  }

  /**
   * Every send this connection never got an answer for is marked not sent.
   * Called wherever the connection ends — a close we did not ask for, and every
   * close we did — because the one thing an unacknowledged message must not do
   * is go on saying "sending" for ever (`SPEC.md`, "Session state"). A send
   * that really did land is put back by the replay: the `user_message` carries
   * the same `client_id` and replaces the message wherever it stands.
   */
  private markUnsent(): void {
    if (this.invalidated || this.generation !== sessionStoreGeneration()) {
      // Signed out, or the transcript belongs to a login that has ended:
      // there is nothing left of this session to mark (ADR 0027).
      return;
    }
    peekSessionStore(this.sessionId)?.getState().connectionLost(UNACKNOWLEDGED);
  }

  /** Drops our handlers before closing, so this close never reconnects. */
  private teardown(code: number): void {
    this.markUnsent();
    const socket = this.socket;
    this.socket = null;
    this.terminalRunning = false;
    if (socket === null) return;
    detach(socket);
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
    // Every text frame is checked against the contract before any of it
    // reaches the store (`serverMessage.ts`): a frame that does not match is
    // dropped whole, so nothing is half-folded and `lastSeq` never moves past
    // an event the transcript does not contain. The diagnostic names the field
    // that failed and never its value — the frame is agent output and the URL
    // it came in on carries an access token (`CLAUDE.md` rule 3).
    const outcome = parseServerMessage(data);
    if (!outcome.ok) {
      if (outcome.unknown) {
        // A newer orchestrator; a message carrying no cursor costs nothing to
        // skip (`SPEC.md`, "WebSocket: session stream").
        console.debug("session socket: %s", outcome.detail);
        return;
      }
      console.warn("session socket: frame ignored (%s)", outcome.detail);
      return;
    }
    this.dispatch(outcome.message);
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

  /**
   * Every close the socket did not ask for. Three readings of one event:
   *
   * - an authentication close (`error { authentication required }` and 1008,
   *   `SPEC.md`, "Authentication") is the one close a token rotation can fix,
   *   and it is tried exactly once per window: a token this browser has just
   *   refreshed being refused again is not a token problem, and rotating the
   *   single-use refresh cookie on every close is how a dead stream turned
   *   into a rotation storm;
   * - any other close leaves the credentials alone — an open stream is not
   *   closed because its token expired, so a close is no evidence about it —
   *   and simply comes back, on backoff when the attempt never opened;
   * - and either way the attempts are counted. A connection that never opens
   *   is a refused upgrade whose status JavaScript never sees, so the session
   *   is read over REST — which does answer in words — at the first such
   *   close and again at `ATTEMPTS_BEFORE_CHECK`, and a deleted or forbidden
   *   session ends here visibly instead of retrying at the backoff cap for
   *   ever (`retryOrCheck`).
   */
  private handleClose(code: number): void {
    this.socket = null;
    this.terminalRunning = false;
    if (this.dead) return;
    this.markUnsent();

    if (this.wasOpen) {
      // It opened, so this is a live connection that dropped: the counters of
      // the previous failure run mean nothing any more.
      this.failedOpens = 0;
      this.checkedAt = 0;
      this.attempt = 0;
    } else {
      this.failedOpens += 1;
    }

    const authClose = this.sawAuthError || code === AUTH_CLOSE_CODE;
    this.sawAuthError = false;
    if (authClose) {
      const now = Date.now();
      const refreshed =
        this.lastAuthCloseAt !== null &&
        now - this.lastAuthCloseAt < AUTH_RETRY_WINDOW_MS;
      this.lastAuthCloseAt = refreshed ? null : now;
      if (!refreshed) {
        this.store.getState().setStatus("reconnecting");
        void this.refreshThenReconnect();
        return;
      }
      // A freshly refreshed token was refused again. Either this browser's
      // authentication is really gone, or it is fine and the refusal belongs
      // to this session alone — a revoked account and a session read failing
      // inside the socket's re-authorization close identically. REST is the
      // one place that tells them apart.
      this.store.getState().setStatus("reconnecting");
      void this.checkSession("stop", REFUSED);
      return;
    }

    this.retryOrCheck();
  }

  /**
   * What to do after a close of a connection that never opened — and the
   * bound on how long that may go on.
   *
   * The orchestrator refuses the upgrade *before* it happens: the token is
   * answered 401 or 403 and a missing session 404, all of them invisible to
   * JavaScript, which sees a close with code 1006 and no `open` either way
   * (`ARCHITECTURE.md`, "Orchestrator internals"; `src/ws/mod.rs`). An access
   * token that expired while the tab slept and a session somebody deleted are
   * therefore the same event here, and the cheapest way to tell them apart is
   * to read the session over REST: that read goes through `apiClient`, which
   * rotates the access token if and only if it really is expired — and adopts
   * one another caller has already installed rather than rotating again — so
   * the next attempt carries a current token without this socket ever
   * guessing that the credentials were the problem.
   *
   * So: the first close of a run asks, the schedule then backs off, and
   * `ATTEMPTS_BEFORE_CHECK` asks a second time before `ATTEMPTS_BEFORE_OFFLINE`
   * ends it.
   */
  private retryOrCheck(): void {
    if (this.dead) return;
    if (this.failedOpens >= ATTEMPTS_BEFORE_OFFLINE) {
      this.giveUp(UNREACHABLE);
      return;
    }
    this.store.getState().setStatus("reconnecting");
    if (this.wantsCheck()) {
      void this.checkSession("retry", UNREACHABLE);
      return;
    }
    this.scheduleRetry();
  }

  /** Whether this close is one of the two a REST read answers. */
  private wantsCheck(): boolean {
    // A connection that opened settles nothing about the token: it had one
    // the server accepted, and an open stream is not closed because that
    // token later expired (`SPEC.md`, "Authentication").
    if (this.failedOpens === 0) return false;
    if (this.checkedAt === 0) return true;
    return (
      this.failedOpens >= ATTEMPTS_BEFORE_CHECK &&
      this.checkedAt < ATTEMPTS_BEFORE_CHECK
    );
  }

  /**
   * Reads the session over REST to decide what the closes mean — and, by
   * going through `apiClient`, to put a current access token in the hands of
   * the attempt that follows (`retryOrCheck`).
   *
   * `onAlive` is what a session that still reads means here: `retry` for a
   * connection that never opened — the orchestrator is there and it answered
   * us, so the next attempt is worth making — and `stop` for a refusal that
   * survived a token rotation, where it is not.
   */
  private async checkSession(onAlive: Alive, fallback: string): Promise<void> {
    if (this.checking || this.dead) return;
    this.checking = true;
    const first = this.checkedAt === 0 && this.failedOpens === 1;
    this.checkedAt = Math.max(this.failedOpens, 1);
    try {
      await getSession(this.sessionId);
    } catch (error) {
      if (this.dead) return;
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
      // Nothing answered: that is a reason to keep trying, under the same
      // bound as any other attempt.
      console.warn("session check failed:", reason(error));
      this.scheduleRetry();
      return;
    } finally {
      this.checking = false;
    }
    if (this.dead) return;
    if (onAlive === "stop") {
      this.giveUp(fallback);
      return;
    }
    if (first) {
      // The read just came back from the orchestrator with credentials it
      // accepted, so there is nothing to wait for: reopen at once, carrying
      // whatever token that read left current. A second failure right after
      // this one falls through to the backoff below.
      this.connect();
      return;
    }
    this.scheduleRetry();
  }

  /**
   * The end of the line: no timer, no rotation, no pretence that a reconnect
   * is underway — a sentence saying why and a `reconnect()` for the reader to
   * press (`SPEC.md`, "Frontend", "Session state").
   */
  private giveUp(message: string): void {
    this.cancelRetry();
    this.lastAuthCloseAt = null;
    this.teardown(1000);
    this.store.getState().setStatus("offline", message);
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
      if (isUnauthorized(error) || isStaleRefreshError(error)) {
        this.giveUp(SIGNED_OUT);
        return;
      }
      this.retryOrCheck();
      return;
    } finally {
      this.refreshing = false;
    }
    if (this.dead) return;
    if (this.wasOpen) {
      this.attempt = 0;
      this.connect();
      return;
    }
    // The last attempt never opened: back off rather than spin. The rotation
    // just answered the question the first REST read exists for, so that read
    // is not owed; the one at `ATTEMPTS_BEFORE_CHECK` still is.
    this.checkedAt = Math.max(this.checkedAt, 1);
    this.retryOrCheck();
  }

  private scheduleRetry(): void {
    if (this.dead || this.retryTimer !== null) return;
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
    this.reconnect();
  }

  /**
   * The sign-in ended. The stream is closed and nothing is attempted again,
   * and — because `invalidated` is part of `dead` — no request still in
   * flight can fold anything into this session either: whatever it answers
   * belongs to the login that just ended. The registry clears the transcript
   * itself, from its own `onSignOut` (`sessionStore`); what is left here is
   * the sentence saying why the connection stopped.
   */
  private onSignOut(): void {
    this.cancelRetry();
    this.teardown(1000);
    this.invalidated = true;
    this.store.getState().setStatus("offline", SIGNED_OUT);
  }

  /**
   * Start again from nothing: the reader's way out of `offline`, and the path
   * a genuine credential replacement takes. Every count the last run left
   * behind is dropped, so a socket that gave up gets the full schedule again
   * rather than one attempt at the cap.
   */
  reconnect(): void {
    if (this.disposed) return;
    // Whatever the browser holds now is what this connection belongs to: a
    // clear that ran just before this — the credentials-replaced path — left
    // an empty store for this id, and this is the connection that fills it.
    this.invalidated = false;
    this.generation = sessionStoreGeneration();
    this.cancelRetry();
    this.attempt = 0;
    this.failedOpens = 0;
    this.checkedAt = 0;
    this.lastAuthCloseAt = null;
    this.teardown(1000);
    this.store.getState().setStatus("reconnecting");
    this.connect();
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
      void sendInput(this.sessionId, input, clientId).then(
        () => {
          // A 202 is the same answer `input_accepted` is, without a `seq`:
          // the orchestrator has it, so the next close is not what lost it.
          if (this.dead) return;
          this.store.getState().inputAccepted(clientId);
        },
        (error: unknown) => {
          // A refusal that lands after this transcript stopped being ours
          // belongs to nobody: the optimistic message it would mark rejected is
          // already gone.
          if (this.dead) return;
          this.store.getState().inputRejected(clientId, reason(error));
        },
      );
    }
    return clientId;
  }

  stop(): void {
    if (this.sendJson({ type: "stop" })) return;
    void stopSession(this.sessionId).catch((error: unknown) => {
      if (this.dead) return;
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
    if (this.dead) return true;
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
      const page = await listEvents(this.sessionId, { before, limit: HISTORY_PAGE_SIZE });
      // A page of one login's transcript, arriving after that login ended, is
      // exactly what must never be prepended: the store it was asked for has
      // been cleared, and whoever holds the tab now would be reading somebody
      // else's session (ADR 0027).
      if (this.dead) return true;
      this.store.getState().prependHistory(page.events, page.has_more);
      this.store.getState().setHistoryStatus("idle");
      return true;
    } catch (error) {
      // The transcript keeps what it holds; only the status changes, so the
      // reader sees why and can ask again.
      console.warn("older session history failed to load:", reason(error));
      if (this.dead) return false;
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

  /**
   * Closes the terminal, then the socket, cancels pending backoff and gives
   * the store back to the registry, which decides whether to keep it for a
   * return visit (`sessionStore`, the registry).
   *
   * The store it leaves behind says `connecting`, not whatever the connection
   * last was: a retained `live` would have the next visit's header claim a
   * stream that does not exist yet and its terminal attach to a socket that
   * has not opened, and a retained `offline` would show the reason the last
   * visit ended as though it were this one's.
   */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    for (const unsubscribe of this.unsubscribes.splice(0)) unsubscribe();
    this.cancelRetry();
    this.terminal.close();
    this.teardown(1000);
    this.terminalListeners.clear();
    peekSessionStore(this.sessionId)?.getState().setStatus("connecting");
    if (!this.retained) return;
    this.retained = false;
    releaseSessionStore(this.sessionId);
  }
}

export function useSessionSocket(
  sessionId: string,
  factory?: SocketFactory,
): SessionSocketApi {
  const ref = useRef<SessionSocket | null>(null);
  const status = useSessionStore(sessionId, (state) => state.status);
  const error = useSessionStore(sessionId, (state) => state.connectionError);

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
  const reconnect = useCallback(() => {
    socketFor(sessionId).reconnect();
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

  return { status, error, send, stop, reconnect, loadOlder, terminal };
}
