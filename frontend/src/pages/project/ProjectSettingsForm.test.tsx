// What the settings form says after a save (`CLAUDE.md`, "Frontend
// conventions", "Submitting a form": one owner of pending, error and
// "saved", and neither answer outlives what it describes).
//
// The scenario is the one the reset exists for: a save lands, the banner says
// so, and the user then edits a field. The form on screen is no longer the one
// that was saved, so the banner has to go — and a refusal has to go the same
// way, rather than sitting above a form that has since been corrected.

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

import { ApiError } from "../../services/apiClient";
import { listBranches, updateProject } from "../../services/projects";
import type { Project } from "../../types";
import { ProjectSettingsForm } from "./ProjectSettingsForm";

vi.mock("../../services/projects", () => ({
  listBranches: vi.fn(),
  updateProject: vi.fn(),
}));

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "11111111-1111-4111-8111-111111111111";

const PROJECT: Project = {
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
  created_at: "2026-03-01T09:00:00Z",
  has_credential: false,
};

function renderForm() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    // The fields' `Learn more` links are router links.
    <MemoryRouter>
      <QueryClientProvider client={client}>
        <ProjectSettingsForm project={PROJECT} />
      </QueryClientProvider>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.mocked(listBranches).mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("ProjectSettingsForm", () => {
  it("stops saying a save landed once the values change", async () => {
    vi.mocked(updateProject).mockResolvedValue({ ...PROJECT, name: "mars-2" });
    renderForm();

    fireEvent.change(screen.getByLabelText(/^Name/), {
      target: { value: "mars-2" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save settings" }));

    await screen.findByText("Settings saved.");

    fireEvent.change(screen.getByLabelText(/^Name/), {
      target: { value: "mars-3" },
    });
    expect(screen.queryByText("Settings saved.")).toBeNull();
  });

  it("shows the server's refusal and drops it when the form is edited", async () => {
    vi.mocked(updateProject).mockRejectedValueOnce(
      new ApiError(409, "project name is taken"),
    );
    renderForm();

    fireEvent.change(screen.getByLabelText(/^Name/), {
      target: { value: "taken" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save settings" }));

    await screen.findByText("project name is taken");
    expect(screen.queryByText("Settings saved.")).toBeNull();

    fireEvent.change(screen.getByLabelText(/^Name/), {
      target: { value: "free" },
    });
    await waitFor(() => {
      expect(screen.queryByText("project name is taken")).toBeNull();
    });
  });
});
