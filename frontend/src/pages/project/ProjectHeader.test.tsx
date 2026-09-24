// What deleting a project does to the query cache (`SPEC.md`, "Frontend",
// Project page). The page is still mounted while the request answers, so every
// read under `["projects", id]` has to go rather than be invalidated: an
// invalidation is a refetch, and the project is gone.

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
import { deleteProject } from "../../services/projects";
import type { Project } from "../../types";
import { ProjectHeader } from "./ProjectHeader";

vi.mock("../../services/projects", () => ({
  deleteProject: vi.fn(),
  fetchProject: vi.fn(),
  retryClone: vi.fn(),
}));

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "00000000-0000-0000-0000-0000000000a1";

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
    max_rounds: 5,
    max_concurrent_sessions: null,
    automation_paused: false,
    created_at: "2026-01-01T00:00:00Z",
    has_credential: true,
  };
}

let queryClient: QueryClient;

function renderHeader() {
  queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[`/projects/${PROJECT_ID}`]}>
        <ProjectHeader
          project={project()}
          settingsOpen={false}
          onToggleSettings={() => undefined}
        />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.mocked(deleteProject).mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

describe("ProjectHeader", () => {
  it("forgets the deleted project's reads and re-reads only the list", async () => {
    renderHeader();

    // Two reads of the project, as its tabs make them, and the table's own.
    queryClient.setQueryData(["projects", PROJECT_ID], project());
    queryClient.setQueryData(["projects", PROJECT_ID, "sessions"], []);
    queryClient.setQueryData(["projects", "list"], [project()]);

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    fireEvent.click(
      screen.getByRole("button", { name: "Delete project mars" }),
    );

    await waitFor(() => {
      expect(vi.mocked(deleteProject)).toHaveBeenCalledWith(PROJECT_ID);
    });

    await waitFor(() => {
      expect(
        queryClient.getQueryState(["projects", PROJECT_ID]),
      ).toBeUndefined();
    });
    expect(
      queryClient.getQueryState(["projects", PROJECT_ID, "sessions"]),
    ).toBeUndefined();

    // The table is still there, and was told to read again.
    const list = queryClient.getQueryState(["projects", "list"]);
    expect(list).not.toBeUndefined();
    expect(list?.isInvalidated).toBe(true);
  });
});
