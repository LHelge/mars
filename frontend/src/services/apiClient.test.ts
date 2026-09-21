import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ApiError,
  apiDelete,
  apiGet,
  apiPost,
  onForbidden,
  onPasswordChangeRequired,
} from "./apiClient";
import {
  clearAuth,
  getAccessToken,
  installSession,
  onSignOut,
} from "./auth";
import type { AuthResponse, User } from "../types";

const user: User = {
  id: "00000000-0000-0000-0000-000000000001",
  username: "tester",
  email: "tester@example.invalid",
  admin: false,
  must_change_password: false,
  notify_email: true,
  created_at: "2026-01-01T00:00:00Z",
};

function authResponse(token: string): AuthResponse {
  return { user, access_token: token };
}

/**
 * A minimal `Response` stand-in: only the members `apiClient` touches, so the
 * tests do not depend on whichever `Response` implementation the environment
 * happens to expose.
 */
function fakeResponse(
  status: number,
  body?: string,
  statusText = "Status Text",
): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText,
    text: () => Promise.resolve(body ?? ""),
  } as unknown as Response;
}

function jsonResponse(status: number, body: unknown): Response {
  return fakeResponse(status, JSON.stringify(body));
}

function errorResponse(status: number, error: string, conflicts?: string[]) {
  return jsonResponse(status, { status, error, ...(conflicts && { conflicts }) });
}

/** `apiClient` always passes a string URL; this keeps the assertions honest. */
function urlOf(input: RequestInfo | URL): string {
  if (typeof input !== "string") {
    throw new TypeError("apiClient passed a non-string URL");
  }
  return input;
}

const fetchMock = vi.fn<typeof fetch>();

function requestUrls(): string[] {
  return fetchMock.mock.calls.map(([input]) => urlOf(input));
}

function headerOf(call: number, name: string): string | null {
  const init = fetchMock.mock.calls[call][1];
  return new Headers(init?.headers).get(name);
}

beforeEach(() => {
  fetchMock.mockReset();
  globalThis.fetch = fetchMock;
  clearAuth();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("apiClient requests", () => {
  it("prefixes /api, sets credentials and omits the bearer header without a token", async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse(200, { orchestrator: true }));

    await apiGet<{ orchestrator: boolean }>("/health");

    expect(requestUrls()).toEqual(["/api/health"]);
    expect(fetchMock.mock.calls[0][1]?.credentials).toBe("same-origin");
    expect(headerOf(0, "authorization")).toBeNull();
    expect(headerOf(0, "content-type")).toBeNull();
  });

  it("attaches the bearer token and the JSON content type for a body", async () => {
    installSession(authResponse("token-a"));
    fetchMock.mockResolvedValueOnce(jsonResponse(200, user));

    await apiPost<User>("/users", { username: "someone" });

    expect(headerOf(0, "authorization")).toBe("Bearer token-a");
    expect(headerOf(0, "content-type")).toBe("application/json");
    expect(fetchMock.mock.calls[0][1]?.body).toBe(
      JSON.stringify({ username: "someone" }),
    );
  });

  it("resolves 204 and an empty body to undefined", async () => {
    installSession(authResponse("token-a"));
    fetchMock
      .mockResolvedValueOnce(fakeResponse(204))
      .mockResolvedValueOnce(fakeResponse(200, ""));

    await expect(apiDelete("/projects/p1")).resolves.toBeUndefined();
    await expect(apiGet<void>("/projects/p1/noop")).resolves.toBeUndefined();
  });

  it("rejects with an ApiError built from the JSON body", async () => {
    installSession(authResponse("token-a"));
    fetchMock.mockResolvedValueOnce(
      errorResponse(409, "the last admin cannot be removed"),
    );

    const error = await apiDelete("/users/u1").catch((e: unknown) => e);

    expect(error).toBeInstanceOf(ApiError);
    const apiError = error as ApiError;
    expect(apiError.status).toBe(409);
    expect(apiError.error).toBe("the last admin cannot be removed");
    expect(apiError.message).toBe("the last admin cannot be removed");
  });

  it("carries conflicts through and names a non-JSON body by its status", async () => {
    installSession(authResponse("token-a"));
    fetchMock
      .mockResolvedValueOnce(
        errorResponse(422, "merge conflict", ["src/main.rs"]),
      )
      // nginx answers a stopped orchestrator with HTML, and `statusText` is
      // always empty under HTTP/2 and HTTP/3.
      .mockResolvedValueOnce(fakeResponse(502, "<html>bad gateway", ""))
      .mockResolvedValueOnce(fakeResponse(418, "", ""));

    const conflict = (await apiPost("/sessions/s1/merge").catch(
      (e: unknown) => e,
    )) as ApiError;
    expect(conflict.conflicts).toEqual(["src/main.rs"]);

    const gateway = (await apiGet("/projects").catch((e: unknown) => e)) as ApiError;
    expect(gateway.status).toBe(502);
    expect(gateway.error).toBe("Orchestrator unreachable");

    const odd = (await apiGet("/projects").catch((e: unknown) => e)) as ApiError;
    expect(odd.error).toBe("HTTP 418");
  });

  it("surfaces a login 429 as an ApiError so the page can show the throttle message", async () => {
    fetchMock.mockResolvedValueOnce(errorResponse(429, "too many attempts"));

    const error = (await apiPost("/auth/login", {
      username: "tester",
      password: "irrelevant",
    }).catch((e: unknown) => e)) as ApiError;

    expect(error.status).toBe(429);
  });

  it("lets a network TypeError through untouched", async () => {
    const failure = new TypeError("Failed to fetch");
    fetchMock.mockRejectedValueOnce(failure);

    await expect(apiGet("/health")).rejects.toBe(failure);
  });
});

describe("apiClient 401 handling", () => {
  it("refreshes once and retries the original request with the new token", async () => {
    installSession(authResponse("token-a"));
    fetchMock
      .mockResolvedValueOnce(errorResponse(401, "unauthenticated"))
      .mockResolvedValueOnce(jsonResponse(200, authResponse("token-b")))
      .mockResolvedValueOnce(jsonResponse(200, user));

    await expect(apiGet<User>("/users/me")).resolves.toEqual(user);

    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(requestUrls()).toEqual([
      "/api/users/me",
      "/api/auth/refresh",
      "/api/users/me",
    ]);
    expect(headerOf(0, "authorization")).toBe("Bearer token-a");
    expect(headerOf(2, "authorization")).toBe("Bearer token-b");
    expect(getAccessToken()).toBe("token-b");
  });

  it("signs out once when the refresh itself answers 401", async () => {
    installSession(authResponse("token-a"));
    const handler = vi.fn();
    const unregister = onSignOut(handler);
    fetchMock
      .mockResolvedValueOnce(errorResponse(401, "unauthenticated"))
      .mockResolvedValueOnce(errorResponse(401, "refresh token expired"));

    const error = (await apiGet<User>("/users/me").catch(
      (e: unknown) => e,
    )) as ApiError;

    expect(error.status).toBe(401);
    expect(handler).toHaveBeenCalledTimes(1);
    expect(handler).toHaveBeenCalledWith("refresh_failed");
    expect(getAccessToken()).toBeNull();
    expect(globalThis.localStorage.getItem("mars.access_token")).toBeNull();
    unregister();
  });

  it("shares one refresh between concurrent 401s", async () => {
    installSession(authResponse("token-a"));
    fetchMock.mockImplementation((input, init) => {
      const url = urlOf(input);
      if (url === "/api/auth/refresh") {
        return Promise.resolve(jsonResponse(200, authResponse("token-b")));
      }
      const token = new Headers(init?.headers).get("authorization");
      return Promise.resolve(
        token === "Bearer token-b"
          ? jsonResponse(200, user)
          : errorResponse(401, "unauthenticated"),
      );
    });

    await Promise.all([
      apiGet<User>("/users/me"),
      apiGet<User>("/projects"),
      apiGet<User>("/sessions"),
    ]);

    const refreshes = requestUrls().filter((url) => url === "/api/auth/refresh");
    expect(refreshes).toHaveLength(1);
  });

  it("retries without refreshing when the token it carried is no longer current", async () => {
    installSession(authResponse("token-a"));
    let attempts = 0;
    fetchMock.mockImplementation((input) => {
      if (urlOf(input) !== "/api/users/me") {
        return Promise.resolve(jsonResponse(200, authResponse("token-c")));
      }
      attempts += 1;
      if (attempts === 1) {
        // Another request's refresh landed while this one was in flight.
        installSession(authResponse("token-b"));
        return Promise.resolve(errorResponse(401, "unauthenticated"));
      }
      return Promise.resolve(jsonResponse(200, user));
    });

    await expect(apiGet<User>("/users/me")).resolves.toEqual(user);

    // No second rotation of the single-use cookie: just the retry.
    expect(requestUrls()).toEqual(["/api/users/me", "/api/users/me"]);
    expect(headerOf(1, "authorization")).toBe("Bearer token-b");
  });

  it("does not refresh after a second 401 on the retry", async () => {
    installSession(authResponse("token-a"));
    fetchMock
      .mockResolvedValueOnce(errorResponse(401, "unauthenticated"))
      .mockResolvedValueOnce(jsonResponse(200, authResponse("token-b")))
      .mockResolvedValueOnce(errorResponse(401, "unauthenticated"));

    const error = (await apiGet<User>("/users/me").catch(
      (e: unknown) => e,
    )) as ApiError;

    expect(error.status).toBe(401);
    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(getAccessToken()).toBe("token-b");
  });

  it("never refreshes a 401 from an unauthenticated auth path", async () => {
    fetchMock.mockResolvedValueOnce(errorResponse(401, "invalid credentials"));

    await expect(
      apiPost("/auth/login", { username: "tester", password: "wrong" }),
    ).rejects.toBeInstanceOf(ApiError);

    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("keeps the token when the refresh fails with a network error", async () => {
    installSession(authResponse("token-a"));
    const handler = vi.fn();
    const unregister = onSignOut(handler);
    const failure = new TypeError("Failed to fetch");
    fetchMock
      .mockResolvedValueOnce(errorResponse(401, "unauthenticated"))
      .mockRejectedValueOnce(failure);

    await expect(apiGet<User>("/users/me")).rejects.toBe(failure);

    expect(getAccessToken()).toBe("token-a");
    expect(handler).not.toHaveBeenCalled();
    unregister();
  });

  it("keeps the token when the refresh fails with a 5xx", async () => {
    installSession(authResponse("token-a"));
    fetchMock
      .mockResolvedValueOnce(errorResponse(401, "unauthenticated"))
      .mockResolvedValueOnce(errorResponse(500, "internal error"));

    const error = (await apiGet<User>("/users/me").catch(
      (e: unknown) => e,
    )) as ApiError;

    expect(error.status).toBe(500);
    expect(getAccessToken()).toBe("token-a");
  });
});

describe("apiClient 403 handling", () => {
  it("fires onPasswordChangeRequired for the password-change 403", async () => {
    installSession(authResponse("token-a"));
    const passwordChange = vi.fn();
    const forbidden = vi.fn();
    const unregisterPassword = onPasswordChangeRequired(passwordChange);
    const unregisterForbidden = onForbidden(forbidden);
    fetchMock.mockResolvedValueOnce(
      errorResponse(403, "password change required"),
    );

    await expect(apiGet("/projects")).rejects.toBeInstanceOf(ApiError);

    expect(passwordChange).toHaveBeenCalledTimes(1);
    expect(forbidden).not.toHaveBeenCalled();
    unregisterPassword();
    unregisterForbidden();
  });

  it("debounces onForbidden to one call per two seconds", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-01-01T00:00:00Z"));
    installSession(authResponse("token-a"));
    const forbidden = vi.fn();
    const unregister = onForbidden(forbidden);
    fetchMock.mockResolvedValue(errorResponse(403, "admin required"));

    await expect(apiGet("/users")).rejects.toBeInstanceOf(ApiError);
    await expect(apiGet("/users")).rejects.toBeInstanceOf(ApiError);
    expect(forbidden).toHaveBeenCalledTimes(1);

    vi.setSystemTime(new Date("2026-01-01T00:00:03Z"));
    await expect(apiGet("/users")).rejects.toBeInstanceOf(ApiError);
    expect(forbidden).toHaveBeenCalledTimes(2);

    unregister();
  });
});
