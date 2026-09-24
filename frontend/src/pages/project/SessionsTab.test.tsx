// The state filter of the sessions tab (`SPEC.md`, "Frontend", Project page).
// It narrows the one list the tab already has rather than asking the server
// for each state, so no state's rows are ever shown under another's heading
// while a per-state request is in flight.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { listSessionBranches } from "../../services/git";
import { listProfiles } from "../../services/profiles";
import { listBranches } from "../../services/projects";
import { getAgentCredentials } from "../../services/secrets";
import { listProjectSessions } from "../../services/sessions";
import type { Project, Session } from "../../types";
import { SessionsTab } from "./SessionsTab";

vi.mock("../../services/profiles", () => ({ listProfiles: vi.fn() }));
vi.mock("../../services/projects", () => ({ listBranches: vi.fn() }));
vi.mock("../../services/secrets", () => ({ getAgentCredentials: vi.fn() }));
vi.mock("../../services/git", () => ({ listSessionBranches: vi.fn() }));
vi.mock("../../services/sessions", () => ({
  listProjectSessions: vi.fn(),
  createSession: vi.fn(),
  deleteSession: vi.fn(),
}));

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "00000000-0000-4000-8000-0000000000a1";

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

function session(id: string, state: Session["state"], title: string): Session {
  return {
    id,
    project_id: PROJECT_ID,
    profile_id: "00000000-0000-4000-8000-0000000000c3",
    kind: "conversational",
    created_by: "00000000-0000-4000-8000-0000000000e5",
    launch_source: "user",
    title,
    task_id: null,
    handoff_id: null,
    state,
    base_ref: "main",
    branch: "mars/session-1",
    container_id: null,
    cli_session_id: null,
    last_seq: 1,
    last_activity_at: "2026-03-01T11:00:00Z",
    cost_usd: 0,
    input_tokens: 0,
    output_tokens: 0,
    error: null,
    created_at: "2026-03-01T10:00:00Z",
    parked_at: null,
    ended_at: null,
  };
}

function renderTab() {
  render(
    <QueryClientProvider
      client={
        new QueryClient({
          defaultOptions: {
            queries: { retry: false },
            mutations: { retry: false },
          },
        })
      }
    >
      <MemoryRouter initialEntries={[`/projects/${PROJECT_ID}`]}>
        <SessionsTab project={project()} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.mocked(listProfiles).mockResolvedValue([]);
  vi.mocked(listBranches).mockResolvedValue([]);
  vi.mocked(getAgentCredentials).mockResolvedValue([]);
  vi.mocked(listSessionBranches).mockResolvedValue([]);
  vi.mocked(listProjectSessions).mockResolvedValue([
    session("00000000-0000-4000-8000-0000000000d4", "running", "still going"),
    session("00000000-0000-4000-8000-0000000000d5", "failed", "gave up"),
  ]);
});

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

describe("SessionsTab", () => {
  it("filters the one list it has, never showing another state's rows", async () => {
    renderTab();

    expect(await screen.findByText("still going")).not.toBeNull();
    expect(screen.getByText("gave up")).not.toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Failed" }));

    // Synchronously, in the same commit as the click: there is nothing to
    // wait for, so there is no window in which `still going` is a failure.
    expect(screen.queryByText("still going")).toBeNull();
    expect(screen.getByText("gave up")).not.toBeNull();

    // One read, unfiltered — never `?state=failed`.
    for (const call of vi.mocked(listProjectSessions).mock.calls) {
      expect(call[1]).toBeUndefined();
    }
  });

  it("leaves the git panel to the Branches tab", async () => {
    renderTab();

    expect(await screen.findByText("still going")).not.toBeNull();
    expect(screen.queryByRole("heading", { name: "Branches" })).toBeNull();
    expect(vi.mocked(listSessionBranches)).not.toHaveBeenCalled();
  });
});
