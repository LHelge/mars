// The REST layer every scenario talks to the real orchestrator through.
//
// Playwright's `APIRequestContext` is the transport; this module adds the
// `/api` prefix, the bearer header, the `{ status, error }` envelope of
// `SPEC.md`, "REST API", and the fresh-user route of `SPEC.md`, "Test-only
// routes". Response shapes are the mirrors in `src/types/` — nothing is
// redeclared here.

import type { APIRequestContext, APIResponse } from "@playwright/test";

import type { AuthResponse, User } from "../../src/types";
import { apiBaseUrl, randomSuffix } from "./env";

/** The obviously fake password every test user gets (CLAUDE.md, rule 3). */
export const DEFAULT_TEST_PASSWORD = "E2e-password-1234";

/** The cookie the orchestrator issues the refresh token in (`SPEC.md`, "Authentication"). */
export const REFRESH_COOKIE_NAME = "refresh_token";

export interface TestUser {
  id: string;
  username: string;
  email: string;
  password: string;
  access_token: string;
  /** The value of the `refresh_token` cookie, ready for `context.addCookies`. */
  refresh_cookie: string;
}

/** A non-2xx answer, carrying enough of the response to diagnose it from a report. */
export class ApiCallError extends Error {
  readonly status: number;
  readonly body: string;

  constructor(method: string, path: string, status: number, body: string) {
    super(`${method} ${path} → ${status}: ${body}`);
    this.name = "ApiCallError";
    this.status = status;
    this.body = body;
  }
}

export interface CallOptions {
  /** Status codes that are an expected answer here and must not throw. */
  allow?: number[];
}

/** A raw answer, for the scenarios that assert on the status itself. */
export interface RawResponse {
  status: number;
  body: unknown;
  text: string;
}

/**
 * The typed wrapper `api(request, access_token)` returns. Paths are written
 * without the `/api` prefix — `api.get("/users/me")`.
 */
export interface Api {
  get<T = unknown>(path: string, opts?: CallOptions): Promise<T>;
  post<T = unknown>(
    path: string,
    body?: unknown,
    opts?: CallOptions,
  ): Promise<T>;
  put<T = unknown>(path: string, body?: unknown, opts?: CallOptions): Promise<T>;
  patch<T = unknown>(
    path: string,
    body?: unknown,
    opts?: CallOptions,
  ): Promise<T>;
  delete<T = unknown>(
    path: string,
    body?: unknown,
    opts?: CallOptions,
  ): Promise<T>;
  /** Method, path and body straight through, with the status kept. */
  send(
    method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE",
    path: string,
    body?: unknown,
    opts?: CallOptions,
  ): Promise<RawResponse>;
  /** The token these calls authenticate with, for helpers that need it again. */
  readonly accessToken: string;
}

function parseBody(text: string, contentType: string | undefined): unknown {
  if (text === "" || !(contentType ?? "").includes("json")) {
    return undefined;
  }
  return JSON.parse(text) as unknown;
}

async function readResponse(response: APIResponse): Promise<RawResponse> {
  const text = await response.text();
  return {
    status: response.status(),
    body: parseBody(text, response.headers()["content-type"]),
    text,
  };
}

export function api(request: APIRequestContext, accessToken: string): Api {
  const base = apiBaseUrl();

  async function send(
    method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE",
    path: string,
    body?: unknown,
    opts: CallOptions = {},
  ): Promise<RawResponse> {
    const response = await request.fetch(`${base}/api${path}`, {
      method,
      headers: { Authorization: `Bearer ${accessToken}` },
      ...(body === undefined ? {} : { data: body }),
    });
    const result = await readResponse(response);
    const allowed =
      (result.status >= 200 && result.status < 300) ||
      (opts.allow ?? []).includes(result.status);
    if (!allowed) {
      throw new ApiCallError(method, path, result.status, result.text);
    }
    return result;
  }

  async function call<T>(
    method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE",
    path: string,
    body?: unknown,
    opts?: CallOptions,
  ): Promise<T> {
    const result = await send(method, path, body, opts);
    return result.body as T;
  }

  return {
    accessToken,
    send,
    get: (path, opts) => call("GET", path, undefined, opts),
    post: (path, body, opts) => call("POST", path, body, opts),
    put: (path, body, opts) => call("PUT", path, body, opts),
    patch: (path, body, opts) => call("PATCH", path, body, opts),
    delete: (path, body, opts) => call("DELETE", path, body, opts),
  };
}

export interface CreateTestUserOptions {
  admin?: boolean;
  /** Goes into the username, so a failing run says which scenario made the user. */
  prefix?: string;
  password?: string;
}

/**
 * Creates a fresh user through the test-only route and returns its credentials
 * and both tokens (`SPEC.md`, "Test-only routes"; CLAUDE.md, "Testing
 * expectations": fresh users per test). The password and the `example.test`
 * address are obviously fake — no real credentials ever live here (rule 3).
 *
 * Requires an orchestrator built with the `integration-tests` feature, which
 * is what `npm run test:e2e:up` starts.
 */
export async function createTestUser(
  request: APIRequestContext,
  opts: CreateTestUserOptions = {},
): Promise<TestUser> {
  const username = `e2e-${opts.prefix ?? "user"}-${randomSuffix()}`;
  const email = `${username}@example.test`;
  const password = opts.password ?? DEFAULT_TEST_PASSWORD;

  const response = await request.post(`${apiBaseUrl()}/api/test/users`, {
    data: { username, email, password, admin: opts.admin ?? false },
  });
  const result = await readResponse(response);
  if (result.status !== 201) {
    // A duplicate username cannot happen with a random suffix, but the body is
    // the only thing that explains any other refusal.
    throw new ApiCallError("POST", "/test/users", result.status, result.text);
  }

  const auth = result.body as AuthResponse;
  const refreshCookie = readRefreshCookie(response);

  return {
    id: auth.user.id,
    username,
    email,
    password,
    access_token: auth.access_token,
    refresh_cookie: refreshCookie,
  };
}

/** The `refresh_token` value out of the response's `set-cookie` header(s). */
function readRefreshCookie(response: APIResponse): string {
  for (const header of response.headersArray()) {
    if (header.name.toLowerCase() !== "set-cookie") continue;
    for (const cookie of header.value.split("\n")) {
      const match = /(?:^|;\s*)refresh_token=([^;]*)/.exec(cookie);
      if (match) return match[1];
    }
  }
  throw new Error(
    "POST /api/test/users answered 201 without a refresh_token cookie",
  );
}

/** `GET /users/me` as the given user; the shortest possible token check. */
export function currentUser(client: Api): Promise<User> {
  return client.get<User>("/users/me");
}
