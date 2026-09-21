import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../services/apiClient";
import { clearAuth, installSession } from "../services/auth";
import { listProjects } from "../services/projects";
import {
  createSecret,
  listSecrets,
  replaceSecretValue,
} from "../services/secrets";
import { listUsers } from "../services/users";
import type { Project, SecretMeta, User } from "../types";
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
    max_concurrent_sessions: null,
    automation_paused: false,
    created_at: "2026-01-01T00:00:00Z",
    has_credential: true,
  };
}

function secret(overrides: Partial<SecretMeta> = {}): SecretMeta {
  return {
    id: "00000000-0000-0000-0000-0000000000b1",
    scope: "global",
    scope_id: null,
    name: "MY_TOKEN",
    orchestrator_only: false,
    key_version: 1,
    created_by: USER_ID,
    created_at: "2026-03-01T09:00:00Z",
    updated_at: "2026-03-01T09:00:00Z",
    last_used_at: null,
    credential_for: null,
    ...overrides,
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
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
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
    expect(screen.getByLabelText<HTMLInputElement>("Global").checked).toBe(
      true,
    );
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
    expect(screen.getByLabelText<HTMLInputElement>("My secrets").checked).toBe(
      true,
    );
  });

  it("switches scope through the radio group", async () => {
    renderPage();
    await screen.findByLabelText("My secrets");

    fireEvent.click(screen.getByLabelText("My secrets"));

    await waitFor(() => {
      expect(vi.mocked(listSecrets)).toHaveBeenCalledWith({ scope: "user" });
    });
  });

  it("lists agent credentials in the section above and nowhere else", async () => {
    // Each scope the section reads answers for itself: the caller's own user
    // scope holds the subscription token, the global scope an API key beside
    // an ordinary secret (`SPEC.md`, "Frontend", Agent credentials).
    vi.mocked(listSecrets).mockImplementation((params) => {
      if (params.scope === "user" && params.scope_id === undefined) {
        return Promise.resolve([
          secret({
            id: "00000000-0000-0000-0000-0000000000b2",
            scope: "user",
            scope_id: USER_ID,
            name: "CLAUDE_CODE_OAUTH_TOKEN",
            credential_for: "claude",
          }),
        ]);
      }
      if (params.scope === "global") {
        return Promise.resolve([
          secret(),
          secret({
            id: "00000000-0000-0000-0000-0000000000b3",
            name: "ANTHROPIC_API_KEY",
            credential_for: "claude",
          }),
        ]);
      }
      return Promise.resolve([]);
    });

    renderPage();

    // Under their labels, with the scope each applies to.
    const token = await screen.findByRole("row", {
      name: /Claude subscription token/,
    });
    expect(token.textContent).toContain("You");
    const key = screen.getByRole("row", { name: /Anthropic API key/ });
    expect(key.textContent).toContain("Everyone");

    // The general list below is the global scope, and leaves its credential
    // out while keeping the ordinary secret.
    expect(await screen.findByText("MY_TOKEN")).not.toBeNull();
    expect(screen.queryByText("ANTHROPIC_API_KEY")).toBeNull();
    expect(screen.queryByText("CLAUDE_CODE_OAUTH_TOKEN")).toBeNull();
  });

  it("says so when no scope holds a credential", async () => {
    renderPage();

    expect(await screen.findByText("No agent credential")).not.toBeNull();
    expect(
      screen.getByText(/Sessions cannot authenticate without one/),
    ).not.toBeNull();
  });

  it("writes the name the chosen kind dictates, at the chosen scope", async () => {
    vi.mocked(createSecret).mockResolvedValue(
      secret({ name: "ANTHROPIC_API_KEY", credential_for: "claude" }),
    );

    renderPage();
    const form = await screen.findByRole("form", {
      name: "Add agent credential",
    });

    // The name is never typed: picking the kind is what names the secret.
    fireEvent.click(within(form).getByLabelText("Anthropic API key"));
    fireEvent.change(within(form).getByLabelText(/^Value/), {
      target: { value: "fake-api-key-for-tests" },
    });
    fireEvent.submit(form);

    await waitFor(() => {
      expect(vi.mocked(createSecret)).toHaveBeenCalledWith({
        scope: "user",
        name: "ANTHROPIC_API_KEY",
        value: "fake-api-key-for-tests",
      });
    });

    // The value field is cleared once it is stored.
    await waitFor(() => {
      expect(
        within(form).getByLabelText<HTMLInputElement>(/^Value/).value,
      ).toBe("");
    });
  });

  it("shows the agent-credential 409 of the guided form in the server's own words", async () => {
    const refusal =
      "this scope already has an agent credential (CLAUDE_CODE_OAUTH_TOKEN); replace or delete it first";
    vi.mocked(createSecret).mockRejectedValue(new ApiError(409, refusal));

    renderPage();
    const form = await screen.findByRole("form", {
      name: "Add agent credential",
    });

    fireEvent.click(within(form).getByLabelText("Anthropic API key"));
    fireEvent.change(within(form).getByLabelText(/^Value/), {
      target: { value: "fake-api-key-for-tests" },
    });
    fireEvent.submit(form);

    expect(await within(form).findByText(refusal)).not.toBeNull();
  });

  it("shows the agent-credential 409 of a row's replacement too", async () => {
    const refusal =
      "this scope already has an agent credential (ANTHROPIC_API_KEY); replace or delete it first";
    vi.mocked(listSecrets).mockImplementation((params) =>
      Promise.resolve(
        params.scope === "user" && params.scope_id === undefined
          ? [
              secret({
                scope: "user",
                scope_id: USER_ID,
                name: "CLAUDE_CODE_OAUTH_TOKEN",
                credential_for: "claude",
              }),
            ]
          : [],
      ),
    );
    vi.mocked(replaceSecretValue).mockRejectedValue(new ApiError(409, refusal));

    renderPage();

    const row = await screen.findByRole("row", {
      name: /Claude subscription token/,
    });
    fireEvent.click(within(row).getByRole("button", { name: "Replace value" }));
    fireEvent.change(
      screen.getByLabelText("New value", { selector: "input" }),
      { target: { value: "fake-token-for-tests" } },
    );
    fireEvent.click(screen.getByRole("button", { name: "Save value" }));

    expect(await screen.findByText(refusal)).not.toBeNull();

    // Closing the panel with the same button drops what was typed into it.
    const toggle = within(row).getByRole("button", { name: "Replace value" });
    fireEvent.change(
      screen.getByLabelText("New value", { selector: "input" }),
      { target: { value: "fake-token-for-tests" } },
    );
    fireEvent.click(toggle);
    fireEvent.click(toggle);
    expect(
      screen.getByLabelText<HTMLInputElement>("New value", {
        selector: "input",
      }).value,
    ).toBe("");
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
