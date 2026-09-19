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
  onSignOut,
  requestPasswordReset,
  resetPassword,
  setCurrentUser,
  signOut,
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
