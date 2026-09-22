// What a failed read does to the profiles tab (`SPEC.md`, "Frontend", Read
// failures): a refetch that fails over an open editor is a banner, and a first
// read that fails is never an empty list.
//
// And what automation looks like from here (`SPEC.md`, "Frontend", Unattended
// launches and Scheduled profiles): the list says what runs a profile without
// a person, and the editor renders the schedule the server stored — its two
// timestamps in both zones, and its refusals on the field they are about.

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
import { listProfiles, updateProfile } from "../../services/profiles";
import { queryKeys } from "../../services/queryKeys";
import { getAgentCredentials, listSecrets } from "../../services/secrets";
import { listTaskStates } from "../../services/taskStates";
import type { Profile, Project } from "../../types";
import { formatDateTime, formatUtc } from "../../utils/format";
import { PROFILE_AUTOMATION } from "../../utils/testIds";
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
    schedule_cron: null,
    schedule_prompt: null,
    last_scheduled_at: null,
    next_scheduled_at: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
}

/** The same profile, ephemeral and on a schedule the server has answered for. */
function scheduled(): Profile {
  return {
    ...profile(),
    name: "tech-debt-scan",
    kind: "ephemeral",
    is_default: false,
    auto_launch: true,
    schedule_cron: "0 6 * * *",
    schedule_prompt: "Scan the repository for tech debt and file tasks.",
    last_scheduled_at: "2026-02-01T06:00:00Z",
    next_scheduled_at: "2026-02-02T06:00:00Z",
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

  it("marks what runs a profile without a person, in the list", async () => {
    vi.mocked(listProfiles).mockResolvedValue([profile(), scheduled()]);

    renderTab("?tab=profiles");

    const cells = await screen.findAllByTestId(PROFILE_AUTOMATION);
    expect(cells).toHaveLength(2);
    // Nothing launches the first one by itself.
    expect(cells[0]?.textContent).toBe("manual");
    expect(cells[1]?.textContent).toContain("auto-launch");
    expect(cells[1]?.textContent).toContain("schedule");
    // The expression itself is a hover away, so the column stays narrow.
    expect(cells[1]?.querySelector("[title]")?.getAttribute("title")).toContain(
      "The dispatcher may launch this profile",
    );
  });

  it("shows the schedule of an ephemeral profile in UTC and in local time", async () => {
    vi.mocked(listProfiles).mockResolvedValue([scheduled()]);

    renderTab(`?tab=profiles&profile=${PROFILE_ID}`);

    const cron = await screen.findByLabelText<HTMLInputElement>(
      "Cron expression (UTC)",
    );
    expect(cron.value).toBe("0 6 * * *");
    const prompt =
      screen.getByLabelText<HTMLTextAreaElement>("Schedule prompt");
    expect(prompt.value).toContain("Scan the repository");

    // A stored schedule arrives with its checkbox on; unticking it disables
    // both fields without emptying them, which is how a schedule is turned off.
    const toggle = screen.getByLabelText<HTMLInputElement>(
      /Run this profile on a schedule/,
    );
    expect(toggle.checked).toBe(true);
    expect(cron.disabled).toBe(false);
    fireEvent.click(toggle);
    expect(cron.disabled).toBe(true);
    expect(prompt.disabled).toBe(true);
    expect(cron.value).toBe("0 6 * * *");

    // The server's two timestamps, the local one first and the UTC instant
    // beside it — both readable without hovering anything.
    const next = screen.getByLabelText("Next run");
    expect(next.textContent).toContain(formatDateTime("2026-02-02T06:00:00Z"));
    expect(next.textContent).toContain(formatUtc("2026-02-02T06:00:00Z"));
    expect(screen.getByLabelText("Last run").textContent).toContain(
      formatDateTime("2026-02-01T06:00:00Z"),
    );
  });

  it("shows the server's refusal of an expression at the expression", async () => {
    vi.mocked(listProfiles).mockResolvedValue([scheduled()]);
    vi.mocked(updateProfile).mockRejectedValue(
      new ApiError(
        400,
        "schedule_cron is not a valid cron expression: invalid digit",
      ),
    );

    renderTab(`?tab=profiles&profile=${PROFILE_ID}`);

    const cron = await screen.findByLabelText<HTMLInputElement>(
      "Cron expression (UTC)",
    );
    fireEvent.change(cron, { target: { value: "0 6 * * 9" } });
    fireEvent.click(screen.getByRole("button", { name: "Save profile" }));

    // At the field, in the server's own words, and not as a form-wide alert:
    // the bundle carries no cron parser, so this is the only judgement there is.
    const message = await screen.findByText(
      "schedule_cron is not a valid cron expression: invalid digit",
    );
    expect(message.id).toBe("profile-schedule-cron-error");
    expect(cron.getAttribute("aria-invalid")).toBe("true");
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
