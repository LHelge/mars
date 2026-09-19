import { QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createQueryClient } from "../queryClient";
import { ApiError } from "../services/apiClient";
import { clearAuth, getCurrentUser, installSession } from "../services/auth";
import { changePassword, getMe, updateMe } from "../services/users";
import type { User } from "../types";
import { SettingsPage } from "./SettingsPage";

vi.mock("../services/users", async () => {
  const actual =
    await vi.importActual<typeof import("../services/users")>(
      "../services/users",
    );
  return {
    ...actual,
    getMe: vi.fn(),
    updateMe: vi.fn(),
    changePassword: vi.fn(),
  };
});

const getMeMock = vi.mocked(getMe);
const updateMeMock = vi.mocked(updateMe);
const changePasswordMock = vi.mocked(changePassword);

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const user: User = {
  id: "00000000-0000-0000-0000-000000000001",
  username: "tester",
  email: "tester@example.invalid",
  admin: false,
  must_change_password: false,
  notify_email: true,
  created_at: "2026-01-01T00:00:00Z",
};

function LocationProbe() {
  return <span data-testid="location">{useLocation().pathname}</span>;
}

function renderPage() {
  render(
    <QueryClientProvider client={createQueryClient()}>
      <MemoryRouter initialEntries={["/settings"]}>
        <LocationProbe />
        <Routes>
          <Route path="/settings" element={<SettingsPage />} />
          <Route path="*" element={<span>elsewhere</span>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

function checkbox(): HTMLInputElement {
  return screen.getByRole<HTMLInputElement>("checkbox");
}

beforeEach(() => {
  getMeMock.mockReset();
  updateMeMock.mockReset();
  changePasswordMock.mockReset();
  getMeMock.mockResolvedValue(user);
  clearAuth();
  installSession({ user, access_token: "fake-access-token" });
});

afterEach(() => {
  cleanup();
  clearAuth();
});

describe("SettingsPage", () => {
  it("renders the account details from GET /users/me", async () => {
    getMeMock.mockResolvedValue({
      ...user,
      username: "operator",
      email: "operator@example.invalid",
      admin: true,
    });
    renderPage();

    // The username also rides in the page shell's header, so the assertion is
    // about the one in the Account list.
    const shown = await screen.findAllByText("operator");
    expect(shown.some((element) => element.tagName === "DD")).toBe(true);
    expect(screen.getByText("operator@example.invalid")).toBeTruthy();
    expect(screen.getByText("Administrator")).toBeTruthy();
    // The fetched user is propagated to `services/auth` as well.
    await waitFor(() => {
      expect(getCurrentUser()?.admin).toBe(true);
    });
  });

  it("opts out through PATCH /users/me and flips the checkbox at once", async () => {
    const saved = { ...user, notify_email: false };
    let resolveUpdate: (value: User) => void = () => {};
    updateMeMock.mockReturnValue(
      new Promise<User>((resolve) => {
        resolveUpdate = resolve;
      }),
    );
    renderPage();

    await waitFor(() => {
      expect(checkbox().checked).toBe(true);
    });
    fireEvent.click(checkbox());

    // Optimistic: flipped before the request answers.
    await waitFor(() => {
      expect(checkbox().checked).toBe(false);
    });
    expect(updateMeMock).toHaveBeenCalledWith({ notify_email: false });

    resolveUpdate(saved);
    expect(await screen.findByText("Preferences saved")).toBeTruthy();
    await waitFor(() => {
      expect(getCurrentUser()?.notify_email).toBe(false);
    });
  });

  it("rolls the checkbox back and shows the server text on a rejection", async () => {
    updateMeMock.mockRejectedValue(new ApiError(403, "forbidden for you"));
    renderPage();

    await waitFor(() => {
      expect(checkbox().checked).toBe(true);
    });
    fireEvent.click(checkbox());

    expect(await screen.findByText("forbidden for you")).toBeTruthy();
    await waitFor(() => {
      expect(checkbox().checked).toBe(true);
    });
  });

  it("confirms a password change without navigating away", async () => {
    changePasswordMock.mockResolvedValue({
      user,
      access_token: "fake-new-access-token",
    });
    renderPage();

    await screen.findByText("Member since");

    fireEvent.change(screen.getByLabelText("Current password"), {
      target: { value: "fake-current-password" },
    });
    fireEvent.change(screen.getByLabelText("New password"), {
      target: { value: "fake-new-password" },
    });
    fireEvent.change(screen.getByLabelText("Repeat new password"), {
      target: { value: "fake-new-password" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Change password" }));

    expect(
      await screen.findByText(
        "Password changed. Other sessions of your account have been signed out.",
      ),
    ).toBeTruthy();
    expect(screen.getByTestId("location").textContent).toBe("/settings");
    // The page is still usable: the next `GET /users/me` succeeds.
    expect(getMeMock).toHaveBeenCalled();
  });
});
