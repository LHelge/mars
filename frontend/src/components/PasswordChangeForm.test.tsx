import { QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PasswordChangeForm } from "./PasswordChangeForm";
import { createQueryClient } from "../queryClient";
import { ApiError } from "../services/apiClient";
import {
  clearAuth,
  getAccessToken,
  getCurrentUser,
  installSession,
  onCredentialsReplaced,
} from "../services/auth";
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

function renderForm(onSuccess = vi.fn()) {
  render(
    <QueryClientProvider client={createQueryClient()}>
      <PasswordChangeForm onSuccess={onSuccess} />
    </QueryClientProvider>,
  );
  return onSuccess;
}

function fill(current: string, password: string, confirm = password) {
  fireEvent.change(screen.getByLabelText("Current password"), {
    target: { value: current },
  });
  fireEvent.change(screen.getByLabelText("New password"), {
    target: { value: password },
  });
  fireEvent.change(screen.getByLabelText("Repeat new password"), {
    target: { value: confirm },
  });
  fireEvent.click(screen.getByRole("button", { name: "Change password" }));
}

function value(label: string): string {
  return screen.getByLabelText<HTMLInputElement>(label).value;
}

beforeEach(() => {
  changeMock.mockReset();
  clearAuth();
  installSession({ user, access_token: "old" });
});

afterEach(() => {
  cleanup();
  clearAuth();
});

describe("PasswordChangeForm", () => {
  it("installs the replacement pair and reports success", async () => {
    const replaced = vi.fn();
    const off = onCredentialsReplaced(replaced);
    changeMock.mockResolvedValue({
      user: { ...user, must_change_password: false },
      access_token: "new",
    });

    const onSuccess = renderForm();
    fill("old password!", "correct horse battery");

    await waitFor(() => {
      expect(onSuccess).toHaveBeenCalledTimes(1);
    });
    expect(changeMock).toHaveBeenCalledWith(user.id, {
      current_password: "old password!",
      password: "correct horse battery",
    });
    expect(getAccessToken()).toBe("new");
    expect(getCurrentUser()?.must_change_password).toBe(false);
    expect(replaced).toHaveBeenCalledTimes(1);
    expect(replaced).toHaveBeenCalledWith("new");

    // All three fields are cleared after a success.
    expect(value("Current password")).toBe("");
    expect(value("New password")).toBe("");
    expect(value("Repeat new password")).toBe("");

    off();
  });

  it("shows a rejected current password and keeps the token", async () => {
    changeMock.mockRejectedValue(
      new ApiError(400, "current password is incorrect"),
    );

    const onSuccess = renderForm();
    fill("wrong password", "correct horse battery");

    await waitFor(() => {
      expect(screen.getByText("Current password is incorrect")).toBeDefined();
    });
    expect(onSuccess).not.toHaveBeenCalled();
    expect(getAccessToken()).toBe("old");
    // Only the new-password fields are cleared after a failure.
    expect(value("Current password")).toBe("wrong password");
    expect(value("New password")).toBe("");
    expect(value("Repeat new password")).toBe("");
  });

  it("shows any other 400 verbatim", async () => {
    changeMock.mockRejectedValue(new ApiError(400, "password is too common"));

    renderForm();
    fill("old password!", "correct horse battery");

    await waitFor(() => {
      expect(screen.getByText("password is too common")).toBeDefined();
    });
  });

  it("reports an unreachable orchestrator", async () => {
    changeMock.mockRejectedValue(new TypeError("Failed to fetch"));

    renderForm();
    fill("old password!", "correct horse battery");

    await waitFor(() => {
      expect(screen.getByText("Orchestrator unreachable")).toBeDefined();
    });
  });

  it("blocks a short password and a mismatch before sending", () => {
    renderForm();

    fill("old password!", "123456789");
    expect(screen.getByText("Password must be 10–128 characters")).toBeDefined();
    expect(changeMock).not.toHaveBeenCalled();

    fill("old password!", "correct horse battery", "correct horse batteries");
    expect(screen.getByText("Passwords do not match")).toBeDefined();
    expect(changeMock).not.toHaveBeenCalled();
  });

  it("requires the current password when the field is asked for", () => {
    renderForm();
    fill("", "correct horse battery");

    expect(screen.getByText("Enter your current password")).toBeDefined();
    expect(changeMock).not.toHaveBeenCalled();
  });

  it("omits the current password when requireCurrent is false", async () => {
    changeMock.mockResolvedValue({
      user: { ...user, must_change_password: false },
      access_token: "new",
    });

    render(
      <QueryClientProvider client={createQueryClient()}>
        <PasswordChangeForm requireCurrent={false} />
      </QueryClientProvider>,
    );
    fireEvent.change(screen.getByLabelText("New password"), {
      target: { value: "correct horse battery" },
    });
    fireEvent.change(screen.getByLabelText("Repeat new password"), {
      target: { value: "correct horse battery" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Change password" }));

    await waitFor(() => {
      expect(changeMock).toHaveBeenCalledWith(user.id, {
        password: "correct horse battery",
      });
    });
    expect(screen.queryByLabelText("Current password")).toBeNull();
  });
});
