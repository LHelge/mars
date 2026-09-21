// The guard on `Clear` and `Remove` (`SPEC.md`, "Frontend", Project page):
// the tab reads the project's session list itself, so a session that ends
// while this tab is the one on screen re-enables both buttons.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearAuth, installSession } from "../../services/auth";
import { listSharedDirs } from "../../services/projects";
import { listProjectSessions } from "../../services/sessions";
import type { Project, Session, SharedDir } from "../../types";
import { SharedDirsTab } from "./SharedDirsTab";

vi.mock("../../services/projects", () => ({
  listSharedDirs: vi.fn(),
  createSharedDir: vi.fn(),
  clearSharedDir: vi.fn(),
  deleteSharedDir: vi.fn(),
}));
vi.mock("../../services/sessions", () => ({ listProjectSessions: vi.fn() }));

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "00000000-0000-0000-0000-0000000000a1";
const SESSION_ID = "00000000-0000-0000-0000-0000000000d4";
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
    created_at: "2026-01-01T00:00:00Z",
    has_credential: true,
  };
}

function sharedDir(): SharedDir {
  return {
    name: "target",
    container_path: "/work/target",
    created_at: "2026-03-01T09:00:00Z",
  };
}

function session(state: Session["state"]): Session {
  return {
    id: SESSION_ID,
    project_id: PROJECT_ID,
    profile_id: "00000000-0000-0000-0000-0000000000e5",
    kind: "ephemeral",
    created_by: USER_ID,
    title: "Build it",
    task_id: null,
    handoff_id: null,
    state,
    base_ref: "main",
    branch: "mars/session-1",
    container_id: null,
    cli_session_id: null,
    last_seq: 3,
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
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[`/projects/${PROJECT_ID}?tab=shared-dirs`]}>
        <SharedDirsTab project={project()} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return queryClient;
}

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
  vi.mocked(listSharedDirs).mockResolvedValue([sharedDir()]);
});

afterEach(() => {
  cleanup();
  clearAuth();
  vi.resetAllMocks();
});

describe("SharedDirsTab", () => {
  it("re-enables the two actions once its own read of the sessions shows none live", async () => {
    // The tab is the only thing watching: the sessions tab is unmounted, so a
    // cached list read once at mount would stay stale for as long as the user
    // stays here.
    vi.mocked(listProjectSessions)
      .mockResolvedValueOnce([session("running")])
      .mockResolvedValue([session("done")]);

    const queryClient = renderTab();

    const clear = await screen.findByRole("button", { name: "Clear" });
    const remove = screen.getByRole("button", { name: "Remove" });
    await waitFor(() => {
      expect(clear.hasAttribute("disabled")).toBe(true);
    });
    expect(remove.hasAttribute("disabled")).toBe(true);

    // What the poll does, without waiting for its interval.
    await queryClient.refetchQueries({
      queryKey: ["projects", PROJECT_ID, "sessions"],
    });

    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: "Clear" }).hasAttribute("disabled"),
      ).toBe(false);
    });
    expect(
      screen.getByRole("button", { name: "Remove" }).hasAttribute("disabled"),
    ).toBe(false);
    expect(screen.queryByText(/A session is running/)).toBeNull();
  });

  it("disables nothing while the session list has not answered", async () => {
    // A read that never resolves: the 409 stays the only authority.
    vi.mocked(listProjectSessions).mockReturnValue(new Promise(() => {}));

    renderTab();

    const clear = await screen.findByRole("button", { name: "Clear" });
    expect(clear.hasAttribute("disabled")).toBe(false);
    expect(
      screen.getByRole("button", { name: "Remove" }).hasAttribute("disabled"),
    ).toBe(false);
  });
});
