import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, describe, expect, it } from "vitest";
import { clearAuth, installSession } from "../services/auth";
import type { User } from "../types";
import { AdminRoute } from "./AdminRoute";

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

function renderAdmin() {
  render(
    <MemoryRouter initialEntries={["/admin"]}>
      <Routes>
        <Route element={<AdminRoute />}>
          <Route path="/admin" element={<p>admin page</p>} />
        </Route>
      </Routes>
    </MemoryRouter>,
  );
}

afterEach(() => {
  cleanup();
  clearAuth();
});

describe("AdminRoute", () => {
  it("explains the refusal to a non-admin instead of redirecting", () => {
    installSession({ user: user(), access_token: "fake-access-token" });
    renderAdmin();

    expect(screen.getByRole("alert").textContent).toBe(
      "Administrator access required",
    );
    expect(screen.queryByText("admin page")).toBeNull();
  });

  it("renders the outlet for an admin", () => {
    installSession({
      user: user({ admin: true }),
      access_token: "fake-access-token",
    });
    renderAdmin();

    expect(screen.getByText("admin page")).toBeDefined();
  });
});
