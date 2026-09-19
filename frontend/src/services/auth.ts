// Authentication state and the `/api/auth` endpoints (`SPEC.md`,
// "Authentication", "Auth (`/api/auth`)" and "Frontend").
//
// The access token lives in memory and is mirrored to `localStorage` so a page
// reload stays signed in; the user object is not persisted — the router reloads
// `GET /users/me` at authenticated startup. Deliberately framework-free (a
// plain module object and a `Set` of listeners, not Zustand) so `apiClient` can
// import it without pulling in React.

import type {
  AcceptInviteRequest,
  AuthResponse,
  InviteLookup,
  LoginRequest,
  User,
} from "../types";
import { ApiError, apiGet, apiPost } from "./apiClient";

/** The one key this module writes; nothing else goes to storage. */
const TOKEN_STORAGE_KEY = "mars.access_token";

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
 * Rotates the refresh cookie for a new pair. A 401 means the refresh token is
 * missing, expired or revoked: sign out instead of retrying. A network error or
 * 5xx is transient — the token stays in place and callers back off normally
 * (`SPEC.md`, "Authentication").
 */
export async function refreshAccessToken(): Promise<AuthResponse> {
  try {
    const auth = await apiPost<AuthResponse>("/auth/refresh");
    installSession(auth);
    return auth;
  } catch (error) {
    if (error instanceof ApiError && error.status === 401) {
      signOut("refresh_failed");
    }
    throw error;
  }
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
