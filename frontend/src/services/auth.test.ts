import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  acceptInvite,
  clearAuth,
  getAccessToken,
  getAuthState,
  getCurrentUser,
  installSession,
  isAuthenticated,
  login,
  logout,
  lookupInvite,
  onCredentialsReplaced,
  onSignOut,
  refreshAccessToken,
  requestPasswordReset,
  resetPassword,
  setCurrentUser,
  signOut,
  StaleRefreshError,
  subscribe,
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

const TOKEN_KEY = "mars.access_token";

function authResponse(token: string): AuthResponse {
  return { user, access_token: token };
}

function fakeResponse(status: number, body?: string): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: "Status Text",
    text: () => Promise.resolve(body ?? ""),
  } as unknown as Response;
}

function jsonResponse(status: number, body: unknown): Response {
  return fakeResponse(status, JSON.stringify(body));
}

/** `apiClient` always passes a string URL; this keeps the assertions honest. */
function urlOf(input: RequestInfo | URL): string {
  if (typeof input !== "string") {
    throw new TypeError("apiClient passed a non-string URL");
  }
  return input;
}

function errorResponse(status: number, error: string): Response {
  return jsonResponse(status, { status, error });
}

/**
 * A promise whose settlement the test controls, so a refresh can be held open
 * across a logout or a newer login and released afterwards.
 */
function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const fetchMock = vi.fn<typeof fetch>();

beforeEach(() => {
  fetchMock.mockReset();
  globalThis.fetch = fetchMock;
  clearAuth();
});

describe("auth state", () => {
  it("installs a session, persists only the token and notifies listeners", () => {
    const listener = vi.fn();
    const unsubscribe = subscribe(listener);

    installSession(authResponse("token-a"));

    expect(listener).toHaveBeenCalledTimes(1);
    expect(getAccessToken()).toBe("token-a");
    expect(getCurrentUser()).toEqual(user);
    expect(isAuthenticated()).toBe(true);
    expect(globalThis.localStorage.getItem(TOKEN_KEY)).toBe("token-a");
    // The user object is never persisted; the router reloads `/users/me`.
    expect(Object.keys(globalThis.localStorage)).toEqual([TOKEN_KEY]);

    unsubscribe();
    clearAuth();
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("fires onCredentialsReplaced only for an already signed-in browser", () => {
    const replaced = vi.fn();
    const off = onCredentialsReplaced(replaced);

    // A first sign-in installs credentials, it does not replace any.
    installSession(authResponse("token-a"));
    expect(replaced).not.toHaveBeenCalled();

    // A self-service password change does.
    installSession(authResponse("token-b"), "password_change");
    expect(replaced).toHaveBeenCalledTimes(1);
    expect(replaced).toHaveBeenCalledWith("token-b");
    // The new token is already in place when the handler runs.
    expect(getAccessToken()).toBe("token-b");

    off();
    installSession(authResponse("token-c"), "password_change");
    expect(replaced).toHaveBeenCalledTimes(1);
  });

  it("installs a rotated pair without replacing credentials", () => {
    const replaced = vi.fn();
    const off = onCredentialsReplaced(replaced);
    installSession(authResponse("token-a"));

    // The ordinary 401-then-refresh rotation: a new access token for the same
    // credentials, so nothing reconnects (`SPEC.md`, "Authentication").
    installSession(authResponse("token-b"), "refresh");

    expect(replaced).not.toHaveBeenCalled();
    // A stream that connects later still reads the current token.
    expect(getAccessToken()).toBe("token-b");
    off();
  });

  it("keeps the snapshot object identical until something changes", () => {
    installSession(authResponse("token-a"));
    const snapshot = getAuthState();

    expect(getAuthState()).toBe(snapshot);

    setCurrentUser({ ...user, admin: true });
    expect(getAuthState()).not.toBe(snapshot);
    expect(getAuthState().accessToken).toBe("token-a");
    expect(getAuthState().user?.admin).toBe(true);
  });

  it("clears memory and storage on clearAuth", () => {
    installSession(authResponse("token-a"));

    clearAuth();

    expect(getAccessToken()).toBeNull();
    expect(getCurrentUser()).toBeNull();
    expect(isAuthenticated()).toBe(false);
    expect(globalThis.localStorage.getItem(TOKEN_KEY)).toBeNull();
  });

  it("runs sign-out handlers once and stays safe when a handler signs out again", () => {
    installSession(authResponse("token-a"));
    const reentrant = vi.fn(() => {
      signOut("user");
    });
    const unregister = onSignOut(reentrant);

    signOut("refresh_failed");

    expect(reentrant).toHaveBeenCalledTimes(1);
    expect(reentrant).toHaveBeenCalledWith("refresh_failed");
    expect(getAccessToken()).toBeNull();

    unregister();
    signOut("user");
    expect(reentrant).toHaveBeenCalledTimes(1);
  });
});

describe("auth endpoints", () => {
  it("installs the pair returned by login", async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse(200, authResponse("token-a")));

    await login({ username: "tester", password: "not-a-real-password" });

    expect(urlOf(fetchMock.mock.calls[0][0])).toBe("/api/auth/login");
    expect(getAccessToken()).toBe("token-a");
  });

  it("installs the pair returned by accept-invite", async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse(201, authResponse("token-a")));

    await acceptInvite({
      token: "invite-token",
      username: "tester",
      password: "not-a-real-password",
    });

    expect(urlOf(fetchMock.mock.calls[0][0])).toBe("/api/auth/accept-invite");
    expect(getAccessToken()).toBe("token-a");
  });

  it("clears local state even when the logout request fails", async () => {
    installSession(authResponse("token-a"));
    const handler = vi.fn();
    const unregister = onSignOut(handler);
    fetchMock.mockRejectedValueOnce(new TypeError("Failed to fetch"));

    await logout();

    expect(urlOf(fetchMock.mock.calls[0][0])).toBe("/api/auth/logout");
    expect(getAccessToken()).toBeNull();
    expect(globalThis.localStorage.getItem(TOKEN_KEY)).toBeNull();
    expect(handler).toHaveBeenCalledWith("user");
    unregister();
  });

  it("encodes the invite token into the lookup path", async () => {
    fetchMock.mockResolvedValueOnce(
      jsonResponse(200, {
        email: "invitee@example.invalid",
        admin: false,
        expires_at: "2026-01-08T00:00:00Z",
      }),
    );

    const lookup = await lookupInvite("a b/c");

    expect(urlOf(fetchMock.mock.calls[0][0])).toBe("/api/auth/invite/a%20b%2Fc");
    expect(lookup.email).toBe("invitee@example.invalid");
  });

  it("rotates without running the credentials-replaced handlers", async () => {
    installSession(authResponse("token-a"));
    const replaced = vi.fn();
    const unregister = onCredentialsReplaced(replaced);
    fetchMock.mockResolvedValueOnce(jsonResponse(200, authResponse("token-b")));

    await expect(refreshAccessToken()).resolves.toEqual(
      authResponse("token-b"),
    );

    expect(getAccessToken()).toBe("token-b");
    expect(replaced).not.toHaveBeenCalled();
    unregister();
  });

  it("signs out when the rotation itself answers 401", async () => {
    installSession(authResponse("token-a"));
    const signedOut = vi.fn();
    const unregister = onSignOut(signedOut);
    fetchMock.mockResolvedValueOnce(
      errorResponse(401, "authentication required"),
    );

    await expect(refreshAccessToken()).rejects.toBeInstanceOf(Error);

    expect(signedOut).toHaveBeenCalledTimes(1);
    expect(signedOut).toHaveBeenCalledWith("refresh_failed");
    expect(getAccessToken()).toBeNull();
    unregister();
  });

  it("posts the password reset request and the reset itself", async () => {
    fetchMock
      .mockResolvedValueOnce(fakeResponse(204))
      .mockResolvedValueOnce(fakeResponse(204));

    await requestPasswordReset("tester");
    await resetPassword("reset-token", "not-a-real-password");

    expect(urlOf(fetchMock.mock.calls[0][0])).toBe(
      "/api/auth/request-password-reset",
    );
    expect(fetchMock.mock.calls[0][1]?.body).toBe(
      JSON.stringify({ identifier: "tester" }),
    );
    expect(urlOf(fetchMock.mock.calls[1][0])).toBe("/api/auth/reset-password");
    expect(fetchMock.mock.calls[1][1]?.body).toBe(
      JSON.stringify({ token: "reset-token", password: "not-a-real-password" }),
    );
  });
});

// The cookie `POST /auth/refresh` rotates is single-use: a second submission
// of it fails *and* clears the cookie, so the whole frontend — HTTP retries,
// the session WebSocket and the task SSE stream — shares one rotation, and a
// rotation that no longer owns the browser's authentication installs nothing
// (`SPEC.md`, "Frontend", Rules).
describe("refreshAccessToken coordination", () => {
  it("shares one rotation between an HTTP caller and a stream caller", async () => {
    installSession(authResponse("token-a"));
    const gate = deferred<Response>();
    fetchMock.mockImplementation((input) => {
      expect(urlOf(input)).toBe("/api/auth/refresh");
      return gate.promise;
    });

    const fromHttp = refreshAccessToken();
    const fromStream = refreshAccessToken();
    gate.resolve(jsonResponse(200, authResponse("token-b")));

    await expect(fromHttp).resolves.toEqual(authResponse("token-b"));
    await expect(fromStream).resolves.toEqual(authResponse("token-b"));
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(getAccessToken()).toBe("token-b");

    // The next caller, after that one settled, rotates again.
    fetchMock.mockResolvedValue(jsonResponse(200, authResponse("token-c")));
    await expect(refreshAccessToken()).resolves.toEqual(
      authResponse("token-c"),
    );
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("installs nothing when a rotation succeeds after a logout", async () => {
    installSession(authResponse("token-a"));
    const signedOut = vi.fn();
    const unregisterSignOut = onSignOut(signedOut);
    const replaced = vi.fn();
    const unregisterReplaced = onCredentialsReplaced(replaced);
    const gate = deferred<Response>();
    fetchMock.mockImplementation((input) =>
      urlOf(input) === "/api/auth/refresh"
        ? gate.promise
        : Promise.resolve(fakeResponse(204)),
    );

    const pending = refreshAccessToken();
    await logout();
    gate.resolve(jsonResponse(200, authResponse("token-b")));

    await expect(pending).rejects.toBeInstanceOf(StaleRefreshError);
    expect(getAccessToken()).toBeNull();
    expect(globalThis.localStorage.getItem(TOKEN_KEY)).toBeNull();
    expect(replaced).not.toHaveBeenCalled();
    expect(signedOut).toHaveBeenCalledTimes(1);
    expect(signedOut).toHaveBeenCalledWith("user");
    unregisterSignOut();
    unregisterReplaced();
  });

  it("does not sign out again when a rotation 401s after a logout", async () => {
    installSession(authResponse("token-a"));
    const signedOut = vi.fn();
    const unregister = onSignOut(signedOut);
    const gate = deferred<Response>();
    fetchMock.mockImplementation((input) =>
      urlOf(input) === "/api/auth/refresh"
        ? gate.promise
        : Promise.resolve(fakeResponse(204)),
    );

    const pending = refreshAccessToken();
    await logout();
    gate.resolve(errorResponse(401, "authentication required"));

    await expect(pending).rejects.toBeInstanceOf(StaleRefreshError);
    expect(signedOut).toHaveBeenCalledTimes(1);
    expect(signedOut).toHaveBeenCalledWith("user");
    unregister();
  });

  it("hands a rotation that completes after a newer login the newer pair", async () => {
    installSession(authResponse("token-a"));
    const replaced = vi.fn();
    const unregisterReplaced = onCredentialsReplaced(replaced);
    const signedOut = vi.fn();
    const unregisterSignOut = onSignOut(signedOut);
    const gate = deferred<Response>();
    fetchMock.mockImplementation((input) =>
      urlOf(input) === "/api/auth/refresh"
        ? gate.promise
        : Promise.resolve(jsonResponse(200, authResponse("token-c"))),
    );

    const pending = refreshAccessToken();
    await login({ username: "tester", password: "not-a-real-password" });
    gate.resolve(jsonResponse(200, authResponse("token-b")));

    // The obsolete pair is never installed; the caller reconnects once, with
    // the credentials the login left behind.
    await expect(pending).resolves.toEqual(authResponse("token-c"));
    expect(getAccessToken()).toBe("token-c");
    expect(replaced).toHaveBeenCalledTimes(1);
    expect(replaced).toHaveBeenCalledWith("token-c");
    expect(signedOut).not.toHaveBeenCalled();
    unregisterReplaced();
    unregisterSignOut();
  });

  it("does not sign out a newer login when the obsolete rotation 401s", async () => {
    installSession(authResponse("token-a"));
    const signedOut = vi.fn();
    const unregister = onSignOut(signedOut);
    const gate = deferred<Response>();
    fetchMock.mockImplementation((input) =>
      urlOf(input) === "/api/auth/refresh"
        ? gate.promise
        : Promise.resolve(jsonResponse(200, authResponse("token-c"))),
    );

    const pending = refreshAccessToken();
    await login({ username: "tester", password: "not-a-real-password" });
    // The loser's 401 carries the clearing `Set-Cookie`; it says nothing about
    // the session the login just installed.
    gate.resolve(errorResponse(401, "authentication required"));

    await expect(pending).resolves.toEqual(authResponse("token-c"));
    expect(getAccessToken()).toBe("token-c");
    expect(signedOut).not.toHaveBeenCalled();
    unregister();
  });

  it("keeps the token on a transient failure and rotates again next time", async () => {
    installSession(authResponse("token-a"));
    const signedOut = vi.fn();
    const unregister = onSignOut(signedOut);
    const failure = new TypeError("Failed to fetch");
    fetchMock
      .mockRejectedValueOnce(failure)
      .mockResolvedValueOnce(jsonResponse(200, authResponse("token-b")));

    await expect(refreshAccessToken()).rejects.toBe(failure);
    expect(getAccessToken()).toBe("token-a");
    expect(signedOut).not.toHaveBeenCalled();

    await expect(refreshAccessToken()).resolves.toEqual(
      authResponse("token-b"),
    );
    expect(fetchMock).toHaveBeenCalledTimes(2);
    unregister();
  });
});
