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
 * Registers a handler to run when the token pair of an *already signed-in*
 * browser is replaced — a self-service password change or a refresh rotation
 * (`SPEC.md`, "Authentication": "A successful self-service password change
 * installs its new pair before reopening streams"; "Frontend", Rules:
 * "Self-service password changes replace the token pair and reconnect streams
 * without replaying pending inputs").
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
 * subscribers, and finally — when this browser was already signed in — the
 * `onCredentialsReplaced` handlers, which see the new token already in place.
 */
export function installSession(auth: AuthResponse): void {
  const replaced = state.accessToken !== null;
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

/** Revokes the refresh token server-side, then clears local state regardless. */
export async function logout(): Promise<void> {
  try {
    await apiPost<void>("/auth/logout");
  } catch {
    // The cookie may already be gone or the orchestrator unreachable; the
    // local session goes away either way.
  }
  signOut("user");
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

async function runRefresh(
  generation: number,
  id: number,
): Promise<AuthResponse> {
  try {
    const auth = await apiPost<AuthResponse>("/auth/refresh");
    if (generation !== authGeneration) {
      return currentCredentials();
    }
    installSession(auth);
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
 * A 401 means the refresh token is missing, expired or revoked: sign out
 * instead of retrying. A network error or 5xx is transient — the token stays in
 * place and callers back off normally (`SPEC.md`, "Authentication"). A
 * completion that no longer owns the browser's authentication installs nothing:
 * it resolves with the credentials that replaced it, or rejects with
 * `StaleRefreshError` when the user is signed out.
 */
export function refreshAccessToken(): Promise<AuthResponse> {
  if (refreshAttempt !== null && refreshAttempt.generation === authGeneration) {
    return refreshAttempt.promise;
  }
  const generation = authGeneration;
  const id = nextRefreshId++;
  const promise = runRefresh(generation, id);
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
