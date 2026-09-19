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
import { ApiError } from "../../services/apiClient";
import { clearAuth, installSession } from "../../services/auth";
import {
  createSecret,
  deleteSecret,
  listSecretUses,
  listSecrets,
  patchSecret,
  replaceSecretValue,
} from "../../services/secrets";
import { listUsers } from "../../services/users";
import type { SecretMeta, SecretUse, User } from "../../types";
import { SecretsManager } from "./SecretsManager";

vi.mock("../../services/secrets", () => ({
  listSecrets: vi.fn(),
  createSecret: vi.fn(),
  replaceSecretValue: vi.fn(),
  patchSecret: vi.fn(),
  deleteSecret: vi.fn(),
  listSecretUses: vi.fn(),
}));
vi.mock("../../services/users", () => ({ listUsers: vi.fn() }));

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "00000000-0000-0000-0000-0000000000a1";
const USER_ID = "00000000-0000-0000-0000-0000000000c3";
const SECRET_ID = "00000000-0000-0000-0000-0000000000d4";
const SESSION_ID = "00000000-0000-0000-0000-0000000000e5";
const FAKE_VALUE = "fake-secret-value";

function secret(overrides: Partial<SecretMeta> = {}): SecretMeta {
  return {
    id: SECRET_ID,
    scope: "project",
    scope_id: PROJECT_ID,
    name: "MY_TOKEN",
    orchestrator_only: false,
    key_version: 2,
    created_by: USER_ID,
    created_at: "2026-03-01T09:00:00Z",
    updated_at: "2026-03-01T10:00:00Z",
    last_used_at: null,
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

function renderManager(scope: "project" | "user" = "project"): QueryClient {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        {scope === "project" ? (
          <SecretsManager scope="project" scopeId={PROJECT_ID} />
        ) : (
          <SecretsManager scope="user" scopeId="00000000-0000-0000-0000-00000000beef" />
        )}
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return client;
}

function fillCreateForm(name: string, value: string) {
  fireEvent.change(screen.getByLabelText(/^Name/), { target: { value: name } });
  fireEvent.change(screen.getByLabelText(/^Value/), {
    target: { value },
  });
  fireEvent.click(screen.getByRole("button", { name: "Add secret" }));
}

beforeEach(() => {
  vi.mocked(listSecrets).mockResolvedValue([]);
  vi.mocked(listUsers).mockResolvedValue([user()]);
  vi.mocked(listSecretUses).mockResolvedValue([]);
  installSession({ user: user(), access_token: "test-access-token" });
});

afterEach(() => {
  cleanup();
  clearAuth();
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

describe("SecretsManager", () => {
  it("lists the metadata of the scope, with `never` for an unused secret", async () => {
    vi.mocked(listSecrets).mockResolvedValue([
      secret({ name: "ZULU" }),
      secret({
        id: "00000000-0000-0000-0000-0000000000aa",
        name: "ALPHA",
        orchestrator_only: true,
        last_used_at: "2026-03-01T11:00:00Z",
      }),
    ]);

    renderManager();

    const table = await screen.findByRole("table");
    const names = within(table)
      .getAllByRole("row")
      .slice(1)
      .map((row) => within(row).getAllByRole("cell")[0].textContent);
    // Sorted by name, not by the order the API answered in.
    expect(names[0]).toContain("ALPHA");
    expect(names[1]).toContain("ZULU");

    expect(within(table).getByText("never")).toBeDefined();
    expect(within(table).getAllByText("v2").length).toBe(2);
    // `created_by` resolved through the users list.
    expect(within(table).getAllByText("operator").length).toBe(2);
    expect(
      screen.getByText(/project secrets override global ones/),
    ).toBeDefined();
  });

  it("creates a secret and keeps no value afterwards", async () => {
    vi.mocked(createSecret).mockResolvedValue(secret({ name: "MY_TOKEN_2" }));

    const client = renderManager();
    await screen.findByRole("button", { name: "Add secret" });

    fillCreateForm("my_token_2", FAKE_VALUE);

    await waitFor(() => {
      expect(vi.mocked(createSecret)).toHaveBeenCalledWith({
        scope: "project",
        scope_id: PROJECT_ID,
        name: "MY_TOKEN_2",
        value: FAKE_VALUE,
        orchestrator_only: false,
      });
    });

    await waitFor(() => {
      expect(
        screen.getByLabelText<HTMLTextAreaElement>(/^Value/).value,
      ).toBe("");
    });
    expect(screen.getByLabelText<HTMLInputElement>(/^Name/).value).toBe("");

    // The mutation cache holds a mutation's variables, which here is the body
    // with the plaintext: nothing may be left of it once the request settled.
    await waitFor(() => {
      expect(
        JSON.stringify(
          client
            .getMutationCache()
            .getAll()
            .map((mutation) => mutation.state.variables),
        ),
      ).not.toContain(FAKE_VALUE);
    });
  });

  it("refuses a name the column would refuse without calling the API", async () => {
    renderManager();
    await screen.findByRole("button", { name: "Add secret" });

    fillCreateForm("1abc", FAKE_VALUE);

    expect(
      await screen.findByText(/uppercase letters, digits and underscores/),
    ).toBeDefined();
    expect(vi.mocked(createSecret)).not.toHaveBeenCalled();
  });

  it("names the duplicate on a 409", async () => {
    vi.mocked(createSecret).mockRejectedValue(
      new ApiError(409, "secret already exists"),
    );

    renderManager();
    await screen.findByRole("button", { name: "Add secret" });

    fillCreateForm("MY_TOKEN", FAKE_VALUE);

    expect(
      await screen.findByText(
        "A secret with that name already exists in this scope.",
      ),
    ).toBeDefined();
  });

  it("replaces a value through `PUT /secrets/{id}`", async () => {
    vi.mocked(listSecrets).mockResolvedValue([secret()]);
    vi.mocked(replaceSecretValue).mockResolvedValue(secret());

    renderManager();

    fireEvent.click(
      await screen.findByRole("button", { name: "Replace value" }),
    );
    fireEvent.change(screen.getByLabelText("New value for MY_TOKEN"), {
      target: { value: FAKE_VALUE },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save value" }));

    await waitFor(() => {
      expect(vi.mocked(replaceSecretValue)).toHaveBeenCalledWith(
        SECRET_ID,
        FAKE_VALUE,
      );
    });
    // The textarea is gone once the request succeeded.
    await waitFor(() => {
      expect(screen.queryByLabelText("New value for MY_TOKEN")).toBeNull();
    });
  });

  it("renames and toggles the orchestrator-only flag through `PATCH`", async () => {
    vi.mocked(listSecrets).mockResolvedValue([secret()]);
    vi.mocked(patchSecret).mockResolvedValue(
      secret({ orchestrator_only: true }),
    );

    renderManager();

    fireEvent.click(
      await screen.findByLabelText("Orchestrator only: MY_TOKEN"),
    );
    await waitFor(() => {
      expect(vi.mocked(patchSecret)).toHaveBeenCalledWith(SECRET_ID, {
        orchestrator_only: true,
      });
    });

    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    fireEvent.change(screen.getByLabelText("New name for MY_TOKEN"), {
      target: { value: "other_token" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save name" }));

    await waitFor(() => {
      expect(vi.mocked(patchSecret)).toHaveBeenCalledWith(SECRET_ID, {
        name: "OTHER_TOKEN",
      });
    });
  });

  it("deletes only after the confirmation", async () => {
    vi.mocked(listSecrets).mockResolvedValue([secret()]);
    vi.mocked(deleteSecret).mockResolvedValue(undefined);
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);

    renderManager();

    fireEvent.click(await screen.findByRole("button", { name: "Delete" }));
    expect(confirm).toHaveBeenCalledWith(
      "Delete MY_TOKEN? Sessions launched later will not receive it.",
    );
    expect(vi.mocked(deleteSecret)).not.toHaveBeenCalled();

    confirm.mockReturnValue(true);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => {
      expect(vi.mocked(deleteSecret)).toHaveBeenCalledWith(SECRET_ID);
    });
  });

  it("expands the uses, linking a launch and naming a mirror fetch", async () => {
    vi.mocked(listSecrets).mockResolvedValue([secret()]);
    const uses: SecretUse[] = [
      {
        session_id: SESSION_ID,
        user_id: null,
        purpose: "launch",
        at: "2026-03-01T11:00:00Z",
      },
      {
        session_id: null,
        user_id: null,
        purpose: "git",
        at: "2026-03-01T10:30:00Z",
      },
    ];
    vi.mocked(listSecretUses).mockResolvedValue(uses);

    renderManager();

    fireEvent.click(await screen.findByRole("button", { name: "Uses" }));

    await waitFor(() => {
      expect(vi.mocked(listSecretUses)).toHaveBeenCalledWith(SECRET_ID, 20);
    });
    const link = await screen.findByRole("link", {
      name: SESSION_ID.slice(0, 8),
    });
    expect(link.getAttribute("href")).toBe(`/sessions/${SESSION_ID}`);
    expect(screen.getByText("mirror fetch")).toBeDefined();
  });

  it("explains a 403 on another user's scope and shows no table", async () => {
    vi.mocked(listSecrets).mockRejectedValue(new ApiError(403, "forbidden"));

    renderManager("user");

    expect(
      await screen.findByText("You can only manage your own secrets."),
    ).toBeDefined();
    expect(screen.queryByRole("table")).toBeNull();
  });
});
