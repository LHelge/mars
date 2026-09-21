import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { LoginPage } from "./LoginPage";
import { ApiError } from "../services/apiClient";
import { statusMessage } from "../services/errorMessage";
import { clearAuth, installSession, login } from "../services/auth";
import type { AuthResponse, User } from "../types";

// Only the network call is stubbed; the auth store stays real so the
// already-signed-in redirect is exercised through `useAuth()`.
vi.mock("../services/auth", async () => {
  const actual =
    await vi.importActual<typeof import("../services/auth")>(
      "../services/auth",
    );
  return { ...actual, login: vi.fn() };
});

const loginMock = vi.mocked(login);

const user: User = {
  id: "00000000-0000-0000-0000-000000000001",
  username: "admin",
  email: "admin@example.invalid",
  admin: true,
  must_change_password: false,
  notify_email: true,
  created_at: "2026-01-01T00:00:00Z",
};

function authResponse(mustChange: boolean): AuthResponse {
  return {
    user: { ...user, must_change_password: mustChange },
    access_token: "fake.access.token",
  };
}

/** Prints the destination `LoginPage` carried into the forced change. */
function ChangePasswordProbe() {
  const state: unknown = useLocation().state;
  const from =
    typeof state === "object" && state !== null && "from" in state
      ? String(state.from)
      : "none";
  return <p>change password from {from}</p>;
}

function renderLogin(from?: string) {
  render(
    <MemoryRouter
      initialEntries={[
        { pathname: "/login", state: from === undefined ? null : { from } },
      ]}
    >
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/" element={<p>dashboard</p>} />
        <Route path="/sessions/:id" element={<p>session</p>} />
        <Route path="/change-password" element={<ChangePasswordProbe />} />
      </Routes>
    </MemoryRouter>,
  );
}

function signIn(username = "admin", password = "changeme") {
  fireEvent.change(screen.getByLabelText("Username"), {
    target: { value: username },
  });
  fireEvent.change(screen.getByLabelText("Password"), {
    target: { value: password },
  });
  fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
}

beforeEach(() => {
  clearAuth();
  loginMock.mockReset();
});

afterEach(() => {
  cleanup();
  clearAuth();
});

describe("LoginPage", () => {
  it("navigates to the dashboard after a successful sign-in", async () => {
    loginMock.mockResolvedValue(authResponse(false));

    renderLogin();
    signIn();

    await waitFor(() => {
      expect(screen.getByText("dashboard")).toBeDefined();
    });
    expect(loginMock).toHaveBeenCalledWith({
      username: "admin",
      password: "changeme",
    });
  });

  it("returns to the destination a guard stashed", async () => {
    loginMock.mockResolvedValue(authResponse(false));

    renderLogin("/sessions/abc");
    signIn();

    await waitFor(() => {
      expect(screen.getByText("session")).toBeDefined();
    });
  });

  it("sends a user who must change their password to the change page, keeping the destination", async () => {
    loginMock.mockResolvedValue(authResponse(true));

    renderLogin("/sessions/abc");
    signIn();

    await waitFor(() => {
      expect(
        screen.getByText("change password from /sessions/abc"),
      ).toBeDefined();
    });
  });

  it("reports invalid credentials and clears only the password", async () => {
    loginMock.mockRejectedValue(new ApiError(401, "invalid credentials"));

    renderLogin();
    signIn();

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(
        "Invalid username or password",
      );
    });
    expect(screen.getByLabelText<HTMLInputElement>("Username").value).toBe(
      "admin",
    );
    expect(screen.getByLabelText<HTMLInputElement>("Password").value).toBe("");
  });

  it("reports the throttle window on 429", async () => {
    loginMock.mockRejectedValue(new ApiError(429, "too many requests"));

    renderLogin();
    signIn();

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(
        "Too many failed attempts. Try again in 15 minutes.",
      );
    });
  });

  it("reports an unreachable orchestrator on a network failure", async () => {
    loginMock.mockRejectedValue(new TypeError("Failed to fetch"));

    renderLogin();
    signIn();

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(
        "Orchestrator unreachable",
      );
    });
  });

  it("says something when nginx answers HTML for a stopped orchestrator", async () => {
    // What `apiClient` builds for a 502 whose body is not the `{status,
    // error}` envelope (`services/apiClient.test.ts`). `statusText` is empty
    // under HTTP/2 and HTTP/3, so the alert used to render nothing at all and
    // the button just stopped spinning.
    loginMock.mockRejectedValue(new ApiError(502, statusMessage(502)));

    renderLogin();
    signIn();

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe("Orchestrator unreachable");
    expect(alert.textContent?.length).toBeGreaterThan(0);
  });

  it("shows any other server message verbatim", async () => {
    loginMock.mockRejectedValue(new ApiError(503, "orchestrator restarting"));

    renderLogin();
    signIn();

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(
        "orchestrator restarting",
      );
    });
  });

  it("does not send an empty form", () => {
    renderLogin();
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));

    expect(loginMock).not.toHaveBeenCalled();
    expect(screen.getByText("Enter your username")).toBeDefined();
    expect(screen.getByText("Enter your password")).toBeDefined();
  });

  it("redirects an already-authenticated visitor without rendering the form", () => {
    installSession(authResponse(false));

    renderLogin();

    expect(screen.getByText("dashboard")).toBeDefined();
    expect(screen.queryByLabelText("Username")).toBeNull();
  });

  it("links to the reset flow and says accounts come from invitations", () => {
    renderLogin();

    expect(
      screen
        .getByRole("link", { name: "Forgot your password?" })
        .getAttribute("href"),
    ).toBe("/forgot-password");
    expect(
      screen.getByText("Accounts are created by invitation."),
    ).toBeDefined();
    expect(screen.queryByRole("link", { name: /sign up/i })).toBeNull();
  });
});
