// The single HTTP path for the whole frontend (`CLAUDE.md`, "Frontend
// conventions"): components never call `fetch`, every request goes through
// here. Paths are written without the `/api` prefix — `apiGet("/users/me")`.
//
// `SPEC.md`, "Authentication" and "Frontend": the access token is a 15-minute
// JWT sent as `Authorization: Bearer`, the refresh token is an `HttpOnly`
// cookie that travels with `credentials: "same-origin"`. A 401 on an
// authenticated request refreshes exactly once and retries; a 401 from the
// refresh itself signs out; transient failures keep the token so callers can
// back off normally.
//
// This module and `./auth` import each other (refresh uses the client, the
// client reads the token). Neither calls across the cycle at module load.

import type { ApiErrorBody } from "../types";
import { refreshAccessToken, getAccessToken } from "./auth";

/** The `{ status, error }` envelope of `SPEC.md`, "REST API". */
export class ApiError extends Error {
  readonly status: number;
  readonly error: string;
  /** Present only on git-conflict responses. */
  readonly conflicts?: string[];

  constructor(status: number, error: string, conflicts?: string[]) {
    // The message is the server's `error` so `useFormSubmit` can render
    // `err.message` directly.
    super(error);
    this.name = "ApiError";
    this.status = status;
    this.error = error;
    if (conflicts !== undefined) {
      this.conflicts = conflicts;
    }
  }
}

/** `SPEC.md`, "Authentication": the 403 body that means "change your password". */
const PASSWORD_CHANGE_REQUIRED = "password change required";

/**
 * Endpoints that are reached without an access token, so a 401 from them is
 * the answer and never a reason to refresh.
 */
const UNAUTHENTICATED_PATHS = [
  "/auth/login",
  "/auth/refresh",
  "/auth/logout",
  "/auth/accept-invite",
  "/auth/request-password-reset",
  "/auth/reset-password",
];

function isUnauthenticatedPath(path: string): boolean {
  const withoutQuery = path.split("?")[0];
  return (
    UNAUTHENTICATED_PATHS.includes(withoutQuery) ||
    withoutQuery.startsWith("/auth/invite/")
  );
}

type Handler = () => void;

let forbiddenHandler: Handler | null = null;
let passwordChangeHandler: Handler | null = null;
let lastForbiddenAt = 0;

/** At most one `onForbidden` call per this many milliseconds. */
const FORBIDDEN_DEBOUNCE_MS = 2000;

/**
 * Registers the handler for an authorization 403 (`SPEC.md`, "Frontend":
 * "refresh the current user after an authorization 403 so a demotion updates
 * admin navigation"). Debounced; a no-op until registered.
 */
export function onForbidden(handler: Handler): () => void {
  forbiddenHandler = handler;
  return () => {
    if (forbiddenHandler === handler) {
      forbiddenHandler = null;
    }
  };
}

/** Registers the handler for a 403 whose body is `password change required`. */
export function onPasswordChangeRequired(handler: Handler): () => void {
  passwordChangeHandler = handler;
  return () => {
    if (passwordChangeHandler === handler) {
      passwordChangeHandler = null;
    }
  };
}

function handleForbidden(apiError: ApiError): void {
  if (apiError.error === PASSWORD_CHANGE_REQUIRED) {
    passwordChangeHandler?.();
    return;
  }
  const now = Date.now();
  if (now - lastForbiddenAt < FORBIDDEN_DEBOUNCE_MS) {
    return;
  }
  lastForbiddenAt = now;
  forbiddenHandler?.();
}

/**
 * One in-flight refresh shared by every concurrent 401, so a burst of parallel
 * requests never becomes a refresh storm.
 */
let refreshing: Promise<void> | null = null;

function sharedRefresh(): Promise<void> {
  refreshing ??= refreshAccessToken()
    .then(() => undefined)
    .finally(() => {
      refreshing = null;
    });
  return refreshing;
}

async function parseBody<T>(response: Response): Promise<T> {
  if (response.status === 204) {
    return undefined as T;
  }
  const text = await response.text();
  if (text.length === 0) {
    return undefined as T;
  }
  return JSON.parse(text) as T;
}

async function toApiError(response: Response): Promise<ApiError> {
  const text = await response.text().catch(() => "");
  if (text.length > 0) {
    try {
      const body = JSON.parse(text) as Partial<ApiErrorBody>;
      if (typeof body.error === "string") {
        return new ApiError(
          typeof body.status === "number" ? body.status : response.status,
          body.error,
          body.conflicts,
        );
      }
    } catch {
      // Not the JSON envelope; fall through to the status line.
    }
  }
  return new ApiError(response.status, response.statusText);
}

type Method = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

async function request<T>(
  method: Method,
  path: string,
  body?: unknown,
  init?: RequestInit,
  retried = false,
): Promise<T> {
  const headers = new Headers(init?.headers);
  if (body !== undefined) {
    headers.set("Content-Type", "application/json");
  }
  const token = getAccessToken();
  if (token !== null) {
    headers.set("Authorization", `Bearer ${token}`);
  }

  // A network failure rejects with a `TypeError`, which we let through
  // untouched so callers can show "orchestrator unreachable".
  const response = await fetch(`/api${path}`, {
    ...init,
    method,
    headers,
    // The refresh cookie is `SameSite=Lax`, `Path=/`; same-origin is enough.
    credentials: "same-origin",
    body: body === undefined ? undefined : JSON.stringify(body),
  });

  if (response.status === 401 && !retried && !isUnauthenticatedPath(path)) {
    await sharedRefresh();
    return request<T>(method, path, body, init, true);
  }

  if (!response.ok) {
    const apiError = await toApiError(response);
    if (response.status === 403) {
      handleForbidden(apiError);
    }
    throw apiError;
  }

  return parseBody<T>(response);
}

export function apiGet<T>(path: string, init?: RequestInit): Promise<T> {
  return request<T>("GET", path, undefined, init);
}

export function apiPost<T>(
  path: string,
  body?: unknown,
  init?: RequestInit,
): Promise<T> {
  return request<T>("POST", path, body, init);
}

export function apiPut<T>(
  path: string,
  body?: unknown,
  init?: RequestInit,
): Promise<T> {
  return request<T>("PUT", path, body, init);
}

export function apiPatch<T>(
  path: string,
  body?: unknown,
  init?: RequestInit,
): Promise<T> {
  return request<T>("PATCH", path, body, init);
}

export function apiDelete(path: string, init?: RequestInit): Promise<void> {
  return request<void>("DELETE", path, undefined, init);
}
