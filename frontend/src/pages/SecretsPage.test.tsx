import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearAuth, installSession } from "../services/auth";
import { listProjects } from "../services/projects";
import { listSecrets } from "../services/secrets";
import { listUsers } from "../services/users";
import type { Project, User } from "../types";
import { SecretsPage } from "./SecretsPage";

vi.mock("../services/secrets", () => ({
  listSecrets: vi.fn(),
  createSecret: vi.fn(),
  replaceSecretValue: vi.fn(),
  patchSecret: vi.fn(),
  deleteSecret: vi.fn(),
  listSecretUses: vi.fn(),
}));
vi.mock("../services/projects", () => ({ listProjects: vi.fn() }));
vi.mock("../services/users", () => ({ listUsers: vi.fn() }));

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "00000000-0000-0000-0000-0000000000a1";
const USER_ID = "00000000-0000-0000-0000-0000000000c3";
const OTHER_USER_ID = "00000000-0000-0000-0000-0000000000f7";

function project(): Project {
  return {
    id: PROJECT_ID,
    name: "mars",
    remote_url: "git@example.invalid:acme/mars.git",
    default_branch: "main",
    status: "cloning",
    status_message: null,
    last_fetched_at: null,
    max_attempts: 3,
    created_at: "2026-01-01T00:00:00Z",
    has_credential: true,
  };
}

function user(overrides: Partial<User> = {}): User {
  return {
    id: USER_ID,
    username: "operator",
    email: "operator@example.invalid",
    admin: true,
    must_change_password: false,
    notify_email: true,
    created_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function renderPage(initial = "/secrets") {
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <MemoryRouter initialEntries={[initial]}>
        <SecretsPage />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.mocked(listSecrets).mockResolvedValue([]);
  vi.mocked(listProjects).mockResolvedValue([project()]);
  vi.mocked(listUsers).mockResolvedValue([
    user(),
    user({ id: OTHER_USER_ID, username: "colleague", admin: false }),
  ]);
  installSession({ user: user(), access_token: "test-access-token" });
});

afterEach(() => {
  cleanup();
  clearAuth();
  vi.clearAllMocks();
});

describe("SecretsPage", () => {
  it("reads the global scope by default", async () => {
    renderPage();

    await waitFor(() => {
      expect(vi.mocked(listSecrets)).toHaveBeenCalledWith({ scope: "global" });
    });
    expect(
      screen.getByLabelText<HTMLInputElement>("Global").checked,
    ).toBe(true);
  });

  it("reads the scope out of the URL, including a project still cloning", async () => {
    renderPage(`/secrets?scope=project&scope_id=${PROJECT_ID}`);

    await waitFor(() => {
      expect(vi.mocked(listSecrets)).toHaveBeenCalledWith({
        scope: "project",
        scope_id: PROJECT_ID,
      });
    });
    // A `cloning` project is selectable.
    expect(
      (await screen.findByRole("option", { name: "mars" })).getAttribute(
        "value",
      ),
    ).toBe(PROJECT_ID);
  });

  it("treats the caller's own id as `My secrets` and drops it from the URL", async () => {
    renderPage(`/secrets?scope=user&scope_id=${USER_ID}`);

    await waitFor(() => {
      expect(vi.mocked(listSecrets)).toHaveBeenCalledWith({ scope: "user" });
    });
    expect(
      screen.getByLabelText<HTMLInputElement>("My secrets").checked,
    ).toBe(true);
  });

  it("switches scope through the radio group", async () => {
    renderPage();
    await screen.findByLabelText("My secrets");

    fireEvent.click(screen.getByLabelText("My secrets"));

    await waitFor(() => {
      expect(vi.mocked(listSecrets)).toHaveBeenCalledWith({ scope: "user" });
    });
  });

  it("offers another user only to an administrator", async () => {
    renderPage();
    await screen.findByLabelText("Global");
    expect(screen.queryByLabelText("Another user")).not.toBeNull();

    cleanup();
    clearAuth();
    installSession({
      user: user({ admin: false }),
      access_token: "test-access-token",
    });

    renderPage();
    await screen.findByLabelText("Global");
    expect(screen.queryByLabelText("Another user")).toBeNull();
  });
});
