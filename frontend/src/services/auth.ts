// Authentication state and the `/api/auth` endpoints (`SPEC.md`,
// "Authentication", "Auth (`/api/auth`)" and "Frontend").
//
// The access token lives in memory and is mirrored to `localStorage` so a page
// reload stays signed in; the user object is not persisted — the router reloads
// `GET /users/me` at authenticated startup. Deliberately framework-free (a
// plain module object and a `Set` of listeners, not Zustand) so `apiClient` can
// import it without pulling in React.
//
// This module also owns the one in-flight refresh of the whole frontend: the
// HTTP client, the session WebSocket and the task SSE stream all go through
// `refreshAccessToken()`, so a burst never submits the same rotating cookie
// twice (`SPEC.md`, "Authentication", stream rules).
//
// That mirror to `localStorage` is also how the *tabs* of one browser stay one
// session: a `storage` event is the only notification a tab gets that another
// one signed out or installed a different token, and the same key carries the
// cross-tab half of the single rotation — the Web Locks API serialises the
// rotation itself, and a tab that finds a newer token in storage adopts it
// instead of submitting the single-use cookie a second time.

import type {
  AcceptInviteRequest,
  AuthResponse,
  InviteLookup,
  LoginRequest,
  User,
} from "../types";
import { ApiError, apiGet, apiPost } from "./apiClient";

/**
 * The one key this module writes; nothing else goes to storage. Exported for
 * the Playwright helpers, which seed a browser with an already issued token
 * instead of driving the login form (`frontend/tests/utils/browser.ts`).
 */
export const TOKEN_STORAGE_KEY = "mars.access_token";

export interface AuthState {
  user: User | null;
  accessToken: string | null;
}

export type SignOutReason = "user" | "refresh_failed";

export type SignOutHandler = (reason: SignOutReason) => void;

/** Called with the access token that replaced the previous one. */
export type CredentialsReplacedHandler = (accessToken: string) => void;

/**
 * Why a pair is being installed. It decides whether the
 * `onCredentialsReplaced` handlers run, so open streams survive the one case
 * that is not a change of credentials at all:
 *
 * - `login` — a sign-in or an accepted invite: whoever this browser was, it is
 *   somebody else's session now.
 * - `refresh` — an ordinary rotation of the same session's refresh cookie,
 *   from `apiClient`'s 401 retry or a stream reopening. The access token is
 *   new, the credentials are not: `SPEC.md`, "Authentication" — "an open
 *   stream is not closed merely because that token later expires" — so open
 *   streams are left exactly as they are and pick the fresh token up at their
 *   next connect.
 * - `password_change` — a self-service password change, which revokes every
 *   other pair and hands this browser a replacement; streams reopen with it.
 */
export type InstallReason = "login" | "refresh" | "password_change";

function readStoredToken(): string | null {
  try {
    return globalThis.localStorage.getItem(TOKEN_STORAGE_KEY);
  } catch {
    // Private mode or blocked storage: degrade to memory-only.
    return null;
  }
}

function writeStoredToken(token: string): void {
  try {
    globalThis.localStorage.setItem(TOKEN_STORAGE_KEY, token);
  } catch {
    // Quota or private mode: degrade to memory-only.
  }
}

function removeStoredToken(): void {
  try {
    globalThis.localStorage.removeItem(TOKEN_STORAGE_KEY);
  } catch {
    // Nothing to do; the in-memory token is cleared regardless.
  }
}

// Replaced wholesale on every change so `useSyncExternalStore` can compare by
// identity and re-render only when something actually moved.
let state: AuthState = { user: null, accessToken: readStoredToken() };

// Bumped by every change of *which* credentials this browser holds — a login,
// an accepted invite, an installed refresh or password-change pair, a sign-out.
// A refresh captures it when it starts and compares on completion, so a
// rotation that lost its race cannot reinstall a session that has been
// replaced or dropped meanwhile.
let authGeneration = 0;

const listeners = new Set<() => void>();
const signOutHandlers = new Set<SignOutHandler>();
const credentialsReplacedHandlers = new Set<CredentialsReplacedHandler>();

function setState(next: AuthState): void {
  state = next;
  for (const listener of [...listeners]) {
    listener();
  }
}

/** The stable snapshot object for `useSyncExternalStore`. */
export function getAuthState(): AuthState {
  return state;
}

export function getAccessToken(): string | null {
  return state.accessToken;
}

export function getCurrentUser(): User | null {
  return state.user;
}

export function isAuthenticated(): boolean {
  return state.accessToken !== null;
}

/** Subscribes to auth state changes; returns the unsubscribe function. */
export function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * Registers a handler to run when local authentication is dropped — the router
 * navigates to `/login`, the session and board stores reset and close their
 * streams. Returns the unregister function.
 */
export function onSignOut(handler: SignOutHandler): () => void {
  signOutHandlers.add(handler);
  return () => {
    signOutHandlers.delete(handler);
  };
}

/**
 * Registers a handler to run when the credentials of an *already signed-in*
 * browser are really replaced — a self-service password change, or a login
 * that puts a different session in this browser's place (`SPEC.md`,
 * "Authentication": "A successful self-service password change installs its
 * new pair before reopening streams"; "Frontend", Rules: "Self-service
 * password changes replace the token pair and reconnect streams without
 * replaying pending inputs").
 *
 * An ordinary refresh rotation is *not* such a replacement and does not run
 * these handlers: the access token lives 15 minutes, so any REST 401 rotates
 * it, and tearing a healthy stream down for that would dispose the session's
 * exec PTY — its shell, its working directory and any foreground build — every
 * quarter of an hour.
 *
 * The session transcript and task board stream hooks subscribe to this from
 * their providers and reconnect their event streams with the fresh access
 * token; pending inputs are not replayed. Returns the unregister function.
 */
export function onCredentialsReplaced(
  handler: CredentialsReplacedHandler,
): () => void {
  credentialsReplacedHandlers.add(handler);
  return () => {
    credentialsReplacedHandlers.delete(handler);
  };
}

/**
 * Installs a fresh `{user, access_token}` pair from login, invite, refresh or
 * a self-service password change: token, then user, then the state
 * subscribers, and finally — when this browser was already signed in and the
 * `reason` is a real change of credentials — the `onCredentialsReplaced`
 * handlers, which see the new token already in place.
 *
 * The default is `login`, the reason a caller that installs a pair out of
 * nowhere has; the refresh path and `PasswordChangeForm` name theirs.
 */
export function installSession(
  auth: AuthResponse,
  reason: InstallReason = "login",
): void {
  const replaced = state.accessToken !== null && reason !== "refresh";
  authGeneration += 1;
  writeStoredToken(auth.access_token);
  setState({ user: auth.user, accessToken: auth.access_token });
  if (replaced) {
    for (const handler of [...credentialsReplacedHandlers]) {
      handler(auth.access_token);
    }
  }
}

export function setCurrentUser(user: User): void {
  setState({ user, accessToken: state.accessToken });
}

/**
 * The `sub` claim of an access token, read without verifying anything — the
 * signature, the expiry and `auth_version` are the orchestrator's business and
 * are checked on every request (`SPEC.md`, "Authentication"). The one question
 * answered here is a UI one: is the token another tab just wrote a rotation of
 * *this* session, or somebody else's sign-in? Anything unreadable answers
 * `null`, which the caller treats as "not this session" — the conservative
 * side, where streams reconnect rather than keep running for another account.
 */
function tokenSubject(token: string | null): string | null {
  if (token === null) {
    return null;
  }
  const payload = token.split(".")[1];
  if (payload === undefined || payload.length === 0) {
    return null;
  }
  try {
    const json = globalThis.atob(payload.replace(/-/g, "+").replace(/_/g, "/"));
    const claims: unknown = JSON.parse(json);
    if (typeof claims !== "object" || claims === null || !("sub" in claims)) {
      return null;
    }
    const sub: unknown = claims.sub;
    return typeof sub === "string" ? sub : null;
  } catch {
    // Not a JWT, not base64, not JSON: unknowable, and treated as such.
    return null;
  }
}

/**
 * Takes over a token this tab did not issue — one another tab wrote to
 * `localStorage`, seen through a `storage` event or read again inside the
 * refresh lock. Storage is not written back: the value is already there, and
 * writing it would only re-notify the tab that wrote it.
 *
 * Whether this is a replacement of credentials follows the `sub` claim
 * (`SPEC.md`, "Frontend", Rules): the same subject is the other tab's ordinary
 * rotation, which leaves this tab's session socket, its exec terminal and the
 * task stream exactly where they are, while a different or unreadable subject
 * is another account taking the browser over — the credentials-replaced
 * handlers run and the cached user is dropped, so `GET /users/me` is loaded
 * again for whoever this is now.
 */
function adoptToken(token: string): void {
  if (token === state.accessToken) {
    return;
  }
  const wasSignedIn = state.accessToken !== null;
  const subject = tokenSubject(token);
  const sameSession =
    wasSignedIn &&
    subject !== null &&
    subject === tokenSubject(state.accessToken);
  authGeneration += 1;
  setState({ user: sameSession ? state.user : null, accessToken: token });
  if (wasSignedIn && !sameSession) {
    for (const handler of [...credentialsReplacedHandlers]) {
      handler(token);
    }
  }
}

function handleStorageEvent(event: StorageEvent): void {
  // `key === null` is a `clear()`, which takes the token with it.
  if (event.key !== null && event.key !== TOKEN_STORAGE_KEY) {
    return;
  }
  const stored = event.key === null ? readStoredToken() : event.newValue;
  if (stored === null) {
    // Another tab signed out. Locally only: its `POST /auth/logout` revoked
    // the one refresh cookie this browser has.
    if (state.accessToken !== null) {
      signOut("user");
    }
    return;
  }
  adoptToken(stored);
}

if (typeof globalThis.addEventListener === "function") {
  globalThis.addEventListener("storage", handleStorageEvent);
}

/** Drops the token and user from memory and `localStorage`. */
export function clearAuth(): void {
  authGeneration += 1;
  removeStoredToken();
  setState({ user: null, accessToken: null });
}

let signingOut = false;

/**
 * Clears local authentication and runs every registered handler. Idempotent and
 * safe to call from inside a handler.
 */
export function signOut(reason: SignOutReason): void {
  if (signingOut) {
    return;
  }
  signingOut = true;
  try {
    clearAuth();
    for (const handler of [...signOutHandlers]) {
      handler(reason);
    }
  } finally {
    signingOut = false;
  }
}

export async function login(body: LoginRequest): Promise<AuthResponse> {
  const auth = await apiPost<AuthResponse>("/auth/login", body);
  installSession(auth);
  return auth;
}

/** How long the cookie-revoking request is given before it is abandoned. */
const LOGOUT_TIMEOUT_MS = 5000;

/**
 * An `init` carrying a deadline, where the runtime has one. `AbortSignal` is
 * the only thing that bounds a `fetch`: without it a blackholed orchestrator
 * holds a request until the browser's own connection timeout, minutes later.
 */
function timeoutInit(ms: number): RequestInit | undefined {
  const timeout = (
    AbortSignal as unknown as { timeout?: (ms: number) => AbortSignal }
  ).timeout;
  return typeof timeout === "function"
    ? { signal: timeout.call(AbortSignal, ms) }
    : undefined;
}

/**
 * Signs out locally first, then asks the orchestrator to revoke the refresh
 * cookie. The order matters: the local session — memory, `localStorage`, every
 * sign-out handler and so every other tab — must go away when the user presses
 * the button, not when an unreachable orchestrator gets round to answering,
 * and the request itself is bounded by a timeout so it cannot hang forever.
 * A revocation that never arrives costs the refresh cookie its remaining life;
 * a sign-out that never happens costs the user their session.
 */
export async function logout(): Promise<void> {
  signOut("user");
  try {
    await apiPost<void>(
      "/auth/logout",
      undefined,
      timeoutInit(LOGOUT_TIMEOUT_MS),
    );
  } catch {
    // Already revoked, unreachable, or past the deadline: local authentication
    // is gone either way.
  }
}

/**
 * Thrown by `refreshAccessToken()` when the rotation it awaited finished for
 * credentials this browser no longer holds *and* nothing replaced them — the
 * user signed out, or the refresh cookie was rejected for a session already
 * gone. Callers stop quietly: there is nothing left to reconnect.
 */
export class StaleRefreshError extends Error {
  constructor() {
    super("the refresh completed for a session that no longer exists");
    this.name = "StaleRefreshError";
  }
}

export function isStaleRefreshError(error: unknown): boolean {
  return error instanceof StaleRefreshError;
}

interface RefreshAttempt {
  /** The authentication generation this rotation was started for. */
  generation: number;
  id: number;
  promise: Promise<AuthResponse>;
}

let refreshAttempt: RefreshAttempt | null = null;
let nextRefreshId = 1;

/**
 * The result an obsolete rotation hands its caller: the credentials this
 * browser holds *now*, installed by whatever replaced the ones the rotation was
 * started for. Nothing is installed and no `onCredentialsReplaced` handler runs
 * a second time, so a caller that reconnects on success reconnects once.
 */
function currentCredentials(): AuthResponse {
  if (state.accessToken === null || state.user === null) {
    throw new StaleRefreshError();
  }
  return { user: state.user, access_token: state.accessToken };
}

/** The Web Locks name the rotation is serialised under, across every tab. */
const REFRESH_LOCK = "mars.auth.refresh";

interface LockManagerLike {
  request<T>(name: string, callback: () => Promise<T>): Promise<T>;
}

/**
 * Runs `rotate` under the origin-wide refresh lock where the browser has the
 * Web Locks API, and directly where it has not — an old browser, or jsdom.
 * Without the lock a tab rotates on its own exactly as it did before: two tabs
 * can then still race for the single-use cookie, which is the bug this bounds,
 * not one it introduces.
 */
function withRefreshLock<T>(rotate: () => Promise<T>): Promise<T> {
  const navigator = globalThis.navigator as unknown as
    | { locks?: LockManagerLike }
    | undefined;
  const locks = navigator?.locks;
  if (locks === undefined || typeof locks.request !== "function") {
    return rotate();
  }
  return locks.request(REFRESH_LOCK, rotate);
}

async function runRefresh(
  generation: number,
  id: number,
  startToken: string | null,
): Promise<AuthResponse> {
  try {
    const auth = await withRefreshLock(async () => {
      // Inside the lock: whatever is in storage now is the browser's current
      // token. If another tab rotated while this one waited, its pair is the
      // live one and the cookie this tab would submit is already revoked, so
      // adopt rather than rotate.
      const stored = readStoredToken();
      if (stored !== null && stored !== startToken) {
        adoptToken(stored);
        return null;
      }
      return await apiPost<AuthResponse>("/auth/refresh");
    });
    // `null` is the adopted case, and `adoptToken` has already moved the
    // generation on, so both answers are "the credentials somebody else left".
    if (auth === null || generation !== authGeneration) {
      return currentCredentials();
    }
    installSession(auth, "refresh");
    return auth;
  } catch (error) {
    if (error instanceof StaleRefreshError) {
      throw error;
    }
    if (generation !== authGeneration) {
      // A logout or a newer login replaced these credentials while the
      // rotation was in flight: its failure — including the 401 that carries
      // the clearing `Set-Cookie` — is about a session nobody uses, and must
      // not sign the newer one out.
      return currentCredentials();
    }
    if (error instanceof ApiError && error.status === 401) {
      signOut("refresh_failed");
    }
    throw error;
  } finally {
    if (refreshAttempt?.id === id) {
      refreshAttempt = null;
    }
  }
}

/**
 * Rotates the refresh cookie for a new pair, at most once at a time for the
 * whole frontend: HTTP 401 retries, the session WebSocket and the task SSE
 * stream share one in-flight rotation, because the cookie is single-use and a
 * second submission of it fails (`orchestrator/src/auth/credentials.rs`).
 *
 * The other tabs of the browser hold the same cookie, so the rotation is also
 * serialised across them, under a `navigator.locks` request; inside the lock
 * the stored token is read again and a newer one is adopted instead of
 * rotating. Two tabs whose tokens expire in the same instant therefore produce
 * one rotation, not a winner and a signed-out loser.
 *
 * A 401 means the refresh token is missing, expired or revoked: sign out
 * instead of retrying. A network error or 5xx is transient — the token stays in
 * place and callers back off normally (`SPEC.md`, "Authentication").
 *
 * A rotation installs its pair as `reason: "refresh"`, so an already-open
 * WebSocket or SSE stream is left alone and only the *next* connect reads the
 * new token; a stream that rotated because its own connection died is the one
 * that reconnects, from its own `refreshAccessToken()` call.
 *
 * A completion that no longer owns the browser's authentication installs nothing:
 * it resolves with the credentials that replaced it, or rejects with
 * `StaleRefreshError` when the user is signed out.
 */
export function refreshAccessToken(): Promise<AuthResponse> {
  if (refreshAttempt !== null && refreshAttempt.generation === authGeneration) {
    return refreshAttempt.promise;
  }
  const generation = authGeneration;
  const id = nextRefreshId++;
  const promise = runRefresh(generation, id, state.accessToken);
  refreshAttempt = { generation, id, promise };
  return promise;
}

export function lookupInvite(token: string): Promise<InviteLookup> {
  return apiGet<InviteLookup>(`/auth/invite/${encodeURIComponent(token)}`);
}

export async function acceptInvite(
  body: AcceptInviteRequest,
): Promise<AuthResponse> {
  const auth = await apiPost<AuthResponse>("/auth/accept-invite", body);
  installSession(auth);
  return auth;
}

/** Always resolves with 204; the orchestrator never reveals whether the identifier exists. */
export function requestPasswordReset(identifier: string): Promise<void> {
  return apiPost<void>("/auth/request-password-reset", { identifier });
}

export function resetPassword(token: string, password: string): Promise<void> {
  return apiPost<void>("/auth/reset-password", { token, password });
}
