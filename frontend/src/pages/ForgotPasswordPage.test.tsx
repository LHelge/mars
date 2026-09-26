import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryRouter } from "react-router";
import { ForgotPasswordPage } from "./ForgotPasswordPage";
import { ApiError } from "../services/apiClient";
import { requestPasswordReset } from "../services/auth";

vi.mock("../services/auth", async () => {
  const actual =
    await vi.importActual<typeof import("../services/auth")>(
      "../services/auth",
    );
  return { ...actual, requestPasswordReset: vi.fn() };
});

const requestMock = vi.mocked(requestPasswordReset);

const CONFIRMATION = "If that account exists, a reset link has been sent.";

function renderPage() {
  render(
    <MemoryRouter initialEntries={["/forgot-password"]}>
      <ForgotPasswordPage />
    </MemoryRouter>,
  );
}

function request(identifier = "admin") {
  fireEvent.change(screen.getByLabelText("Username or email"), {
    target: { value: identifier },
  });
  fireEvent.click(screen.getByRole("button", { name: "Send reset link" }));
}

beforeEach(() => {
  requestMock.mockReset();
});

afterEach(cleanup);

describe("ForgotPasswordPage", () => {
  it("confirms after a 204", async () => {
    requestMock.mockResolvedValue(undefined);

    renderPage();
    request();

    await waitFor(() => {
      expect(screen.getByText(CONFIRMATION)).toBeDefined();
    });
    expect(requestMock).toHaveBeenCalledWith("admin");
    expect(
      screen
        .getByRole("link", { name: "Back to sign in" })
        .getAttribute("href"),
    ).toBe("/login");
  });

  it("confirms even when the request is refused, so throttling reveals nothing", async () => {
    // The documented endpoint always answers 204; this is the defensive path.
    requestMock.mockRejectedValue(new ApiError(429, "too many requests"));

    renderPage();
    request();

    await waitFor(() => {
      expect(screen.getByText(CONFIRMATION)).toBeDefined();
    });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("reports only a request that never reached the orchestrator", async () => {
    requestMock.mockRejectedValue(new TypeError("Failed to fetch"));

    renderPage();
    request();

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(
        "Orchestrator unreachable",
      );
    });
    expect(screen.queryByText(CONFIRMATION)).toBeNull();
  });

  it("does not send an empty identifier", () => {
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: "Send reset link" }));

    expect(requestMock).not.toHaveBeenCalled();
    expect(screen.getByText("Enter your username or email")).toBeDefined();
  });
});
