// The projects list, on the one action its rows have: retrying a failed clone
// (`SPEC.md`, "User-facing features"). Each row owns that mutation, so two
// retries in flight are two busy rows, and a refusal is shown on the row it
// was refused on.

import { QueryClientProvider } from "@tanstack/react-query";
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
import { createQueryClient } from "../queryClient";
import { ApiError } from "../services/apiClient";
import { listProjects, retryClone } from "../services/projects";
import type { Project } from "../types";
import { ProjectsPage } from "./ProjectsPage";

vi.mock("../services/projects", async () => {
  const actual = await vi.importActual<typeof import("../services/projects")>(
    "../services/projects",
  );
  return { ...actual, listProjects: vi.fn(), retryClone: vi.fn() };
});

const listProjectsMock = vi.mocked(listProjects);
const retryCloneMock = vi.mocked(retryClone);

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const ALPHA_ID = "00000000-0000-0000-0000-0000000000a1";
const BETA_ID = "00000000-0000-0000-0000-0000000000b2";

function project(overrides: Partial<Project> = {}): Project {
  return {
    id: ALPHA_ID,
    name: "alpha",
    remote_url: "https://git.example.invalid/alpha.git",
    default_branch: null,
    status: "error",
    status_message: "authentication failed",
    last_fetched_at: null,
    max_attempts: 3,
    created_at: "2026-01-01T00:00:00Z",
    has_credential: false,
    ...overrides,
  };
}

function renderPage() {
  render(
    <QueryClientProvider client={createQueryClient()}>
      <MemoryRouter initialEntries={["/projects"]}>
        <ProjectsPage />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The table row a cell's text identifies. */
function rowFor(name: string): HTMLElement {
  const row = screen
    .getAllByRole("row")
    .find((candidate) => within(candidate).queryByText(name) !== null);
  if (row === undefined) {
    throw new Error(`no row for ${name}`);
  }
  return row;
}

function retryIn(name: string): HTMLButtonElement {
  return within(rowFor(name)).getByRole("button", { name: "Retry clone" });
}

beforeEach(() => {
  listProjectsMock.mockReset();
  retryCloneMock.mockReset();
});

afterEach(() => {
  cleanup();
});

describe("ProjectsPage", () => {
  it("keeps both rows busy while two retries overlap", async () => {
    listProjectsMock.mockResolvedValue([
      project(),
      project({ id: BETA_ID, name: "beta" }),
    ]);
    const finish = new Map<string, (value: Project) => void>();
    retryCloneMock.mockImplementation(
      (id: string) =>
        new Promise<Project>((resolve) => {
          finish.set(id, resolve);
        }),
    );
    renderPage();

    await screen.findByText("alpha");
    fireEvent.click(retryIn("alpha"));
    await waitFor(() => {
      expect(finish.has(ALPHA_ID)).toBe(true);
    });
    fireEvent.click(retryIn("beta"));
    await waitFor(() => {
      expect(finish.has(BETA_ID)).toBe(true);
    });

    // A table-wide observer would have followed the second press and
    // re-enabled alpha's button while its request was still in flight.
    expect(retryIn("alpha").disabled).toBe(true);
    expect(retryIn("beta").disabled).toBe(true);
  });

  it("shows a refusal on the row it was refused on", async () => {
    listProjectsMock.mockResolvedValue([
      project(),
      project({ id: BETA_ID, name: "beta" }),
    ]);
    retryCloneMock.mockRejectedValue(
      new ApiError(409, "project is not in the error state"),
    );
    renderPage();

    await screen.findByText("alpha");
    fireEvent.click(retryIn("alpha"));

    const refused = await screen.findByText(
      "project is not in the error state",
    );
    // On alpha's row, and on no other: the answer belongs to the press.
    expect(rowFor("alpha").contains(refused)).toBe(true);
    expect(within(rowFor("beta")).queryByRole("alert")).toBeNull();
  });
});
