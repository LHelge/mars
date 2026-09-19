import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../services/apiClient";
import {
  acceptInvite,
  clearAuth,
  getAccessToken,
  installSession,
  lookupInvite,
} from "../services/auth";
import type { AuthResponse, InviteLookup } from "../types";
import { AcceptInvitePage } from "./AcceptInvitePage";

vi.mock("../services/auth", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../services/auth")>();
  return {
    ...actual,
    lookupInvite: vi.fn(),
    acceptInvite: vi.fn(),
  };
});

const lookupMock = vi.mocked(lookupInvite);
const acceptMock = vi.mocked(acceptInvite);

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const TOKEN = "fake-invite-token";
const ACCESS_TOKEN = "fake.access.token";

const INVALID_LINK =
  "This invitation link is invalid, has expired or was already used. Ask an administrator for a new one.";

function invite(overrides: Partial<InviteLookup> = {}): InviteLookup {
  return {
    email: "newcomer@example.invalid",
    admin: false,
    expires_at: "2026-02-01T10:00:00Z",
    ...overrides,
  };
}

function auth(): AuthResponse {
  return {
    user: {
      id: "00000000-0000-0000-0000-0000000000a1",
      username: "newcomer",
      email: "newcomer@example.invalid",
      admin: false,
      must_change_password: false,
      notify_email: true,
      created_at: "2026-01-25T10:00:00Z",
    },
    access_token: ACCESS_TOKEN,
  };
}

function Location() {
  const location = useLocation();
  return <span data-testid="path">{location.pathname}</span>;
}

function renderPage(path = `/invite/${TOKEN}`) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path="/invite/:token" element={<AcceptInvitePage />} />
          <Route path="/invite" element={<AcceptInvitePage />} />
          <Route path="/" element={<span>dashboard</span>} />
        </Routes>
        <Location />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

function fill(
  username: string,
  password = "correct horse battery",
  confirm = password,
) {
  fireEvent.change(screen.getByLabelText("Username"), {
    target: { value: username },
  });
  fireEvent.change(screen.getByLabelText("Password"), {
    target: { value: password },
  });
  fireEvent.change(screen.getByLabelText("Repeat password"), {
    target: { value: confirm },
  });
  fireEvent.click(screen.getByRole("button", { name: "Create account" }));
}

beforeEach(() => {
  lookupMock.mockReset();
  acceptMock.mockReset();
  clearAuth();
});

afterEach(cleanup);

describe("AcceptInvitePage", () => {
  it("shows the invited email, the expiry and the administrator note", async () => {
    lookupMock.mockResolvedValue(invite({ admin: true }));

    renderPage();

    await waitFor(() => {
      expect(
        screen.getByLabelText<HTMLInputElement>("Invited email").value,
      ).toBe("newcomer@example.invalid");
    });
    expect(lookupMock).toHaveBeenCalledWith(TOKEN);
    expect(screen.getByText("You will be an administrator")).toBeDefined();
    expect(screen.getByText(/^Expires /).textContent).not.toBe("Expires —");
  });

  it("omits the administrator note for an ordinary invite", async () => {
    lookupMock.mockResolvedValue(invite());

    renderPage();

    await screen.findByLabelText("Username");
    expect(screen.queryByText("You will be an administrator")).toBeNull();
  });

  it("shows the dead-end alert when the lookup is rejected", async () => {
    lookupMock.mockRejectedValue(
      new ApiError(400, "invalid or expired invite"),
    );

    renderPage();

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(INVALID_LINK);
    });
    expect(
      screen
        .getByRole("link", { name: "Back to sign in" })
        .getAttribute("href"),
    ).toBe("/login");
    expect(screen.queryByLabelText("Username")).toBeNull();
  });

  it("offers a retry when the orchestrator is unreachable", async () => {
    lookupMock.mockRejectedValueOnce(new TypeError("Failed to fetch"));

    renderPage();

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(
        "Orchestrator unreachable",
      );
    });

    lookupMock.mockResolvedValueOnce(invite());
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));

    await screen.findByLabelText("Username");
  });

  it("offers a retry, not the dead end, when the lookup answers 500", async () => {
    lookupMock.mockRejectedValueOnce(
      new ApiError(500, "internal server error"),
    );

    renderPage();

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(
        "Orchestrator unreachable",
      );
    });
    expect(screen.getByRole("button", { name: "Try again" })).toBeDefined();
  });

  it("shows the dead-end alert without a request when the token is missing", () => {
    renderPage("/invite");

    expect(screen.getByRole("alert").textContent).toBe(INVALID_LINK);
    expect(lookupMock).not.toHaveBeenCalled();
  });

  it("rejects a two-character username client-side", async () => {
    lookupMock.mockResolvedValue(invite());

    renderPage();
    await screen.findByLabelText("Username");
    fill("ab");

    expect(acceptMock).not.toHaveBeenCalled();
    expect(screen.getByText("Username must be 3–32 characters")).toBeDefined();
  });

  it("rejects a mismatched confirmation client-side", async () => {
    lookupMock.mockResolvedValue(invite());

    renderPage();
    await screen.findByLabelText("Username");
    fill("newcomer", "correct horse battery", "correct horse batteru");

    expect(acceptMock).not.toHaveBeenCalled();
    expect(screen.getByText("Passwords do not match")).toBeDefined();
  });

  it("rejects a nine-character password client-side", async () => {
    lookupMock.mockResolvedValue(invite());

    renderPage();
    await screen.findByLabelText("Username");
    fill("newcomer", "123456789");

    expect(acceptMock).not.toHaveBeenCalled();
    expect(
      screen.getByText("Password must be 10–128 characters"),
    ).toBeDefined();
  });

  it("installs the session and lands on the dashboard", async () => {
    lookupMock.mockResolvedValue(invite());
    acceptMock.mockImplementation(() => {
      const response = auth();
      installSession(response);
      return Promise.resolve(response);
    });

    renderPage();
    await screen.findByLabelText("Username");
    fill("newcomer");

    await waitFor(() => {
      expect(screen.getByTestId("path").textContent).toBe("/");
    });
    expect(acceptMock).toHaveBeenCalledWith({
      token: TOKEN,
      username: "newcomer",
      password: "correct horse battery",
    });
    expect(getAccessToken()).toBe(ACCESS_TOKEN);
  });

  it("shows a server 400 verbatim with a way back to sign-in", async () => {
    lookupMock.mockResolvedValue(invite());
    acceptMock.mockRejectedValue(
      new ApiError(400, "invalid or expired invite"),
    );

    renderPage();
    await screen.findByLabelText("Username");
    fill("newcomer");

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(
        "invalid or expired invite",
      );
    });
    expect(
      screen
        .getByRole("link", { name: "Back to sign in" })
        .getAttribute("href"),
    ).toBe("/login");
  });

  it("rewords a 409 as a taken username", async () => {
    lookupMock.mockResolvedValue(invite());
    acceptMock.mockRejectedValue(new ApiError(409, "username already taken"));

    renderPage();
    await screen.findByLabelText("Username");
    fill("newcomer");

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(
        "That username is already taken.",
      );
    });
  });

  it("warns a signed-in visitor that accepting switches accounts", async () => {
    installSession({ ...auth(), access_token: "fake.other.token" });
    lookupMock.mockResolvedValue(invite());

    renderPage();

    await waitFor(() => {
      expect(
        screen.getByText(
          /You are signed in as newcomer; accepting will switch accounts\./,
        ),
      ).toBeDefined();
    });
  });

  it("never echoes the token into the document", async () => {
    lookupMock.mockResolvedValue(invite());

    renderPage();
    await screen.findByLabelText("Username");

    // The `<main>` is the page itself; the probe beside it is the test's own.
    expect(screen.getByRole("main").innerHTML).not.toContain(TOKEN);
    expect(globalThis.localStorage.getItem("mars.access_token")).toBeNull();
  });
});
