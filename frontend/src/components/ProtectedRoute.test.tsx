import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, describe, expect, it } from "vitest";
import { clearAuth, installSession } from "../services/auth";
import type { User } from "../types";
import { useReturnTo } from "../utils/returnTo";
import { ProtectedRoute } from "./ProtectedRoute";

// Obviously fake fixture values (CLAUDE.md, rule 3).
function user(overrides: Partial<User> = {}): User {
  return {
    id: "00000000-0000-0000-0000-000000000001",
    username: "operator",
    email: "operator@example.invalid",
    admin: false,
    must_change_password: false,
    notify_email: true,
    created_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

/** Reports where the router landed and which return destination survived. */
function Probe() {
  const { pathname } = useLocation();
  const from = useReturnTo();
  return (
    <div>
      <span data-testid="path">{pathname}</span>
      <span data-testid="from">{from ?? "none"}</span>
    </div>
  );
}

function renderAt(entry: string) {
  render(
    <MemoryRouter initialEntries={[entry]}>
      <Routes>
        <Route path="/login" element={<Probe />} />
        <Route element={<ProtectedRoute />}>
          <Route path="/change-password" element={<Probe />} />
          <Route path="/projects" element={<p>projects</p>} />
        </Route>
      </Routes>
    </MemoryRouter>,
  );
}

afterEach(() => {
  cleanup();
  clearAuth();
});

describe("ProtectedRoute", () => {
  it("sends an unauthenticated visit to /login with the destination in state", () => {
    clearAuth();
    renderAt("/projects?x=1");

    expect(screen.getByTestId("path").textContent).toBe("/login");
    expect(screen.getByTestId("from").textContent).toBe("/projects?x=1");
  });

  it("sends a must-change-password user to /change-password and keeps the destination", () => {
    installSession({
      user: user({ must_change_password: true }),
      access_token: "fake-access-token",
    });
    renderAt("/projects?x=1");

    expect(screen.getByTestId("path").textContent).toBe("/change-password");
    expect(screen.getByTestId("from").textContent).toBe("/projects?x=1");
  });

  it("does not redirect the must-change-password user away from /change-password", () => {
    installSession({
      user: user({ must_change_password: true }),
      access_token: "fake-access-token",
    });
    renderAt("/change-password");

    expect(screen.getByTestId("path").textContent).toBe("/change-password");
  });

  it("renders the outlet for a signed-in user", () => {
    installSession({ user: user(), access_token: "fake-access-token" });
    renderAt("/projects");

    expect(screen.getByText("projects")).toBeDefined();
  });

  it("omits an unsafe destination from the redirect state", () => {
    clearAuth();
    render(
      <MemoryRouter initialEntries={["/login"]}>
        <Routes>
          <Route path="/login" element={<Probe />} />
        </Routes>
      </MemoryRouter>,
    );

    expect(screen.getByTestId("from").textContent).toBe("none");
  });
});
