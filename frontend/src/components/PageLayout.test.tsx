import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it } from "vitest";
import { clearAuth, installSession } from "../services/auth";
import type { User } from "../types";
import { PageLayout } from "./PageLayout";

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

function renderLayout() {
  render(
    <MemoryRouter initialEntries={["/"]}>
      <PageLayout title="Dashboard">
        <p>body</p>
      </PageLayout>
    </MemoryRouter>,
  );
}

afterEach(() => {
  cleanup();
  clearAuth();
});

describe("PageLayout", () => {
  it("hides the Admin link from a non-admin", () => {
    installSession({ user: user(), access_token: "test-access-token" });
    renderLayout();

    expect(screen.getByRole("link", { name: "Dashboard" })).toBeDefined();
    expect(screen.getByRole("link", { name: "Projects" })).toBeDefined();
    expect(screen.queryByRole("link", { name: "Admin" })).toBeNull();
    expect(screen.getByText("operator")).toBeDefined();
  });

  it("links the help page", () => {
    installSession({ user: user(), access_token: "test-access-token" });
    renderLayout();

    expect(
      screen.getByRole("link", { name: "Help" }).getAttribute("href"),
    ).toBe("/help");
  });

  it("shows the Admin link to an admin", () => {
    installSession({
      user: user({ username: "root-operator", admin: true }),
      access_token: "test-access-token",
    });
    renderLayout();

    expect(screen.getByRole("link", { name: "Admin" })).toBeDefined();
    expect(screen.getByText("root-operator")).toBeDefined();
  });

  it("renders before GET /users/me has resolved", () => {
    clearAuth();
    renderLayout();

    expect(screen.getByRole("button", { name: /Log out/ })).toBeDefined();
    expect(screen.queryByRole("link", { name: "Admin" })).toBeNull();
  });
});
