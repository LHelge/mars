import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryRouter, Route, Routes } from "react-router";
import { ResetPasswordPage } from "./ResetPasswordPage";
import { ApiError } from "../services/apiClient";
import { resetPassword } from "../services/auth";

vi.mock("../services/auth", async () => {
  const actual =
    await vi.importActual<typeof import("../services/auth")>(
      "../services/auth",
    );
  return { ...actual, resetPassword: vi.fn() };
});

const resetMock = vi.mocked(resetPassword);

const INVALID_LINK = "This reset link is invalid or has expired.";

function renderPage(path = "/reset-password/fake-reset-token") {
  render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/reset-password/:token" element={<ResetPasswordPage />} />
        <Route path="/reset-password" element={<ResetPasswordPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

function fill(password: string, confirm = password) {
  fireEvent.change(screen.getByLabelText("New password"), {
    target: { value: password },
  });
  fireEvent.change(screen.getByLabelText("Repeat new password"), {
    target: { value: confirm },
  });
  fireEvent.click(screen.getByRole("button", { name: "Set password" }));
}

beforeEach(() => {
  resetMock.mockReset();
});

afterEach(cleanup);

describe("ResetPasswordPage", () => {
  it("sends the token with the new password and points back to sign-in", async () => {
    resetMock.mockResolvedValue(undefined);

    renderPage();
    fill("correct horse battery");

    await waitFor(() => {
      expect(
        screen.getByText("Password updated. Sign in with your new password."),
      ).toBeDefined();
    });
    expect(resetMock).toHaveBeenCalledWith(
      "fake-reset-token",
      "correct horse battery",
    );
    expect(
      screen.getByRole("link", { name: "Back to sign in" }).getAttribute("href"),
    ).toBe("/login");
  });

  it("rejects a nine-character password without a request", () => {
    renderPage();
    fill("123456789");

    expect(resetMock).not.toHaveBeenCalled();
    expect(screen.getByText("Password must be 10–128 characters")).toBeDefined();
  });

  it("rejects a mismatched confirmation without a request", () => {
    renderPage();
    fill("correct horse battery", "correct horse batteru");

    expect(resetMock).not.toHaveBeenCalled();
    expect(screen.getByText("Passwords do not match")).toBeDefined();
  });

  it("shows the invalid-link alert on a 400", async () => {
    resetMock.mockRejectedValue(new ApiError(400, "invalid or expired token"));

    renderPage();
    fill("correct horse battery");

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(INVALID_LINK);
    });
    expect(
      screen
        .getByRole("link", { name: "Request a new link" })
        .getAttribute("href"),
    ).toBe("/forgot-password");
  });

  it("shows any other 400 verbatim and keeps the form", async () => {
    resetMock.mockRejectedValue(
      new ApiError(400, "password must be 10-128 characters"),
    );

    renderPage();
    fill("correct horse battery");

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(
        "password must be 10-128 characters",
      );
    });
    expect(screen.queryByRole("link", { name: "Request a new link" })).toBeNull();
  });

  it("shows the invalid-link alert immediately without a token", () => {
    renderPage("/reset-password");

    expect(screen.getByRole("alert").textContent).toBe(INVALID_LINK);
    expect(screen.queryByLabelText("New password")).toBeNull();
  });

  it("never echoes the token into the document", () => {
    renderPage();

    expect(document.body.innerHTML).not.toContain("fake-reset-token");
  });
});
