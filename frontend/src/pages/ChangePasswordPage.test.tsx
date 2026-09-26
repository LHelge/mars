import { QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryRouter, Route, Routes } from "react-router";
import { ChangePasswordPage } from "./ChangePasswordPage";
import { createQueryClient } from "../queryClient";
import { clearAuth, installSession } from "../services/auth";
import { changePassword } from "../services/users";
import type { User } from "../types";

vi.mock("../services/users", async () => {
  const actual =
    await vi.importActual<typeof import("../services/users")>(
      "../services/users",
    );
  return { ...actual, changePassword: vi.fn() };
});

const changeMock = vi.mocked(changePassword);

const user: User = {
  id: "00000000-0000-0000-0000-000000000001",
  username: "tester",
  email: "tester@example.invalid",
  admin: false,
  must_change_password: true,
  notify_email: true,
  created_at: "2026-01-01T00:00:00Z",
};

const FORCED_NOTICE = "You must change your password before continuing.";

function renderPage(state?: { from: string }) {
  render(
    <QueryClientProvider client={createQueryClient()}>
      <MemoryRouter initialEntries={[{ pathname: "/change-password", state }]}>
        <Routes>
          <Route path="/change-password" element={<ChangePasswordPage />} />
          <Route path="/" element={<p>dashboard</p>} />
          <Route path="/settings" element={<p>settings</p>} />
          <Route path="/sessions/:id" element={<p>session</p>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

function fill() {
  fireEvent.change(screen.getByLabelText("Current password"), {
    target: { value: "old password!" },
  });
  fireEvent.change(screen.getByLabelText("New password"), {
    target: { value: "correct horse battery" },
  });
  fireEvent.change(screen.getByLabelText("Repeat new password"), {
    target: { value: "correct horse battery" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Change password" }));
}

beforeEach(() => {
  changeMock.mockReset();
  changeMock.mockResolvedValue({
    user: { ...user, must_change_password: false },
    access_token: "new",
  });
  clearAuth();
});

afterEach(() => {
  cleanup();
  clearAuth();
});

describe("ChangePasswordPage", () => {
  it("explains the forced change and continues to the dashboard", async () => {
    installSession({ user, access_token: "old" });

    renderPage();
    expect(screen.getByText("Set a new password")).toBeDefined();
    expect(screen.getByText(FORCED_NOTICE)).toBeDefined();

    fill();

    await waitFor(() => {
      expect(screen.getByText("dashboard")).toBeDefined();
    });
  });

  it("returns to the preserved destination", async () => {
    installSession({ user, access_token: "old" });

    renderPage({ from: "/sessions/abc" });
    fill();

    await waitFor(() => {
      expect(screen.getByText("session")).toBeDefined();
    });
  });

  it("works for a user without the flag and goes back to settings", async () => {
    installSession({
      user: { ...user, must_change_password: false },
      access_token: "old",
    });

    renderPage();
    expect(screen.queryByText(FORCED_NOTICE)).toBeNull();

    fill();

    await waitFor(() => {
      expect(screen.getByText("settings")).toBeDefined();
    });
  });

  it("calls no endpoint other than the password change", async () => {
    installSession({ user, access_token: "old" });
    const fetchSpy = vi.spyOn(globalThis, "fetch");

    renderPage();
    fill();

    await waitFor(() => {
      expect(screen.getByText("dashboard")).toBeDefined();
    });
    expect(fetchSpy).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });
});
