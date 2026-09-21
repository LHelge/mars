// What a failed read does to the profiles tab (`SPEC.md`, "Frontend", Read
// failures): a refetch that fails over an open editor is a banner, and a first
// read that fails is never an empty list.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../../services/apiClient";
import { clearAuth, installSession } from "../../services/auth";
import { listProfiles } from "../../services/profiles";
import { queryKeys } from "../../services/queryKeys";
import { getAgentCredentials, listSecrets } from "../../services/secrets";
import { listTaskStates } from "../../services/taskStates";
import type { Profile, Project } from "../../types";
import { ProfilesTab } from "./ProfilesTab";

vi.mock("../../services/profiles", () => ({
  listProfiles: vi.fn(),
  deleteProfile: vi.fn(),
  createProfile: vi.fn(),
  updateProfile: vi.fn(),
  listProfileTemplates: vi.fn(),
}));
vi.mock("../../services/taskStates", () => ({ listTaskStates: vi.fn() }));
vi.mock("../../services/secrets", () => ({
  listSecrets: vi.fn(),
  getAgentCredentials: vi.fn(),
}));

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "00000000-0000-0000-0000-0000000000a1";
const PROFILE_ID = "00000000-0000-0000-0000-0000000000b2";
const USER_ID = "00000000-0000-0000-0000-0000000000c3";

function project(): Project {
  return {
    id: PROJECT_ID,
    name: "mars",
    remote_url: "git@example.invalid:acme/mars.git",
    default_branch: "main",
    status: "ready",
    status_message: null,
    last_fetched_at: null,
    max_attempts: 3,
    max_concurrent_sessions: null,
    automation_paused: false,
    created_at: "2026-01-01T00:00:00Z",
    has_credential: true,
  };
}

function profile(): Profile {
  return {
    id: PROFILE_ID,
    project_id: PROJECT_ID,
    name: "implementer",
    kind: "conversational",
    backend: "claude",
    image: "ghcr.io/example/mars-session:fake",
    model: null,
    system_prompt: "",
    serves_states: [],
    secrets: [],
    mcp_tools: [],
    runtime: null,
    permission_mode: "bypass",
    partial_messages: true,
    idle_timeout_secs: 900,
    is_default: true,
    auto_launch: false,
    max_concurrent: 1,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
}

let queryClient: QueryClient;

function renderTab(search: string) {
  queryClient = new QueryClient({
    // One failure is one failure in a test: the policy itself is covered by
    // `queryClient.test.ts`.
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[`/projects/${PROJECT_ID}${search}`]}>
        <ProfilesTab project={project()} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("ProfilesTab", () => {
  beforeEach(() => {
    installSession({
      user: {
        id: USER_ID,
        username: "operator",
        email: "operator@example.invalid",
        admin: false,
        must_change_password: false,
        notify_email: false,
        created_at: "2026-01-01T00:00:00Z",
      },
      access_token: "fake-access-token",
    });
    vi.mocked(listTaskStates).mockResolvedValue([]);
    vi.mocked(listSecrets).mockResolvedValue([]);
    vi.mocked(getAgentCredentials).mockResolvedValue([]);
  });

  afterEach(() => {
    cleanup();
    clearAuth();
    vi.resetAllMocks();
  });

  it("keeps a half-written editor when the profiles refetch fails", async () => {
    vi.mocked(listProfiles).mockResolvedValueOnce([profile()]);

    renderTab("?tab=profiles&profile=new");

    // The label carries the required marker: `Name *`.
    const name = await screen.findByLabelText(/^Name/);
    fireEvent.change(name, { target: { value: "scout" } });

    vi.mocked(listProfiles).mockRejectedValueOnce(
      new ApiError(502, "orchestrator unreachable"),
    );
    await act(async () => {
      await queryClient.refetchQueries({
        queryKey: queryKeys.projects.profiles(PROJECT_ID),
      });
    });

    // The failure is said, and nothing else changes: the editor is still
    // mounted with what was typed in it.
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("orchestrator unreachable");
    expect(screen.getByRole("button", { name: "Try again" })).toBeDefined();
    const still = screen.getByLabelText<HTMLInputElement>(/^Name/);
    expect(still.value).toBe("scout");
  });

  it("shows no empty state when the first read fails", async () => {
    vi.mocked(listProfiles).mockRejectedValue(
      new ApiError(502, "orchestrator unreachable"),
    );

    renderTab("?tab=profiles");

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toContain(
        "orchestrator unreachable",
      );
    });
    expect(screen.queryByText("No profiles yet")).toBeNull();
  });
});
