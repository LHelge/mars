// The one boundary check `installSession` owes: an `AuthResponse` is a
// `JSON.parse` cast like any other, and a body without a usable token must not
// become the string "undefined" in `localStorage`.
//
// Kept beside `auth.test.ts` rather than in it: that suite installs sessions in
// almost every case and this one is about refusing to.

import { afterEach, describe, expect, it } from "vitest";

import type { AuthResponse, User } from "../types";
import { clearAuth, getAccessToken, installSession } from "./auth";
import { MessageError } from "./errorMessage";

const TOKEN_KEY = "mars.access_token";

const user: User = {
  id: "00000000-0000-0000-0000-000000000009",
  username: "tester",
  email: "tester@example.invalid",
  admin: false,
  must_change_password: false,
  notify_email: true,
  created_at: "2026-01-01T00:00:00Z",
};

afterEach(() => {
  clearAuth();
  globalThis.localStorage.clear();
});

describe("installSession", () => {
  it("refuses a response with no access token", () => {
    const auth = { user } as unknown as AuthResponse;
    expect(() => {
      installSession(auth);
    }).toThrow(MessageError);
    expect(globalThis.localStorage.getItem(TOKEN_KEY)).toBeNull();
    expect(getAccessToken()).toBeNull();
  });

  it("refuses a token that is not a non-empty string", () => {
    for (const token of [null, 42, ""]) {
      expect(() => {
        installSession({ user, access_token: token } as unknown as AuthResponse);
      }).toThrow(MessageError);
    }
    expect(globalThis.localStorage.getItem(TOKEN_KEY)).toBeNull();
  });

  it("installs a usable one", () => {
    installSession({ user, access_token: "token-a" });
    expect(getAccessToken()).toBe("token-a");
    expect(globalThis.localStorage.getItem(TOKEN_KEY)).toBe("token-a");
  });
});
