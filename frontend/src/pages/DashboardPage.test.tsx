import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearAuth, installSession } from "../services/auth";
import { listProjects } from "../services/projects";
import { listSessions } from "../services/sessions";
import { listHumanTasks } from "../services/tasks";
import { listUsers } from "../services/users";
import type { Project, Session, Task, User } from "../types";
import { DASHBOARD_REFETCH_MS, DashboardPage } from "./DashboardPage";

// `useQuery` is wrapped so a test can read back the options the page passed it
// (the 30-second poll of `SPEC.md`, "Frontend", Dashboard).
vi.mock("@tanstack/react-query", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@tanstack/react-query")>();
  return { ...actual, useQuery: vi.fn(actual.useQuery) };
});

vi.mock("../services/sessions", () => ({ listSessions: vi.fn() }));
vi.mock("../services/tasks", () => ({ listHumanTasks: vi.fn() }));
vi.mock("../services/projects", () => ({ listProjects: vi.fn() }));
vi.mock("../services/users", () => ({ listUsers: vi.fn() }));

// Obviously fake fixture values (CLAUDE.md, rule 3).
const PROJECT_ID = "00000000-0000-0000-0000-0000000000a1";
const OTHER_PROJECT_ID = "00000000-0000-0000-0000-0000000000b2";
const USER_ID = "00000000-0000-0000-0000-0000000000c3";

function project(overrides: Partial<Project> = {}): Project {
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
    ...overrides,
  };
}

function session(overrides: Partial<Session> = {}): Session {
  return {
    id: "00000000-0000-0000-0000-0000000000d4",
    project_id: PROJECT_ID,
    profile_id: "00000000-0000-0000-0000-0000000000e5",
    kind: "conversational",
    created_by: USER_ID,
    launch_source: "user",
    title: "Wire the dashboard",
    task_id: null,
    handoff_id: null,
    state: "running",
    base_ref: "main",
    branch: "mars/session-1",
    container_id: null,
    cli_session_id: null,
    last_seq: 12,
    last_activity_at: "2026-03-01T11:00:00Z",
    cost_usd: 1.5,
    input_tokens: 10,
    output_tokens: 20,
    error: null,
    created_at: "2026-03-01T10:00:00Z",
    parked_at: null,
    ended_at: null,
    ...overrides,
  };
}

function task(overrides: Partial<Task> = {}): Task {
  return {
    id: "00000000-0000-0000-0000-0000000000f6",
    project_id: OTHER_PROJECT_ID,
    number: 42,
    title: "Decide the retention window",
    description: null,
    state: "needs-human",
    priority: 0,
    blocked: false,
    labels: [],
    parent_id: null,
    assignee_user_id: USER_ID,
    lease_holder_session_id: null,
    lease_since: null,
    attempts: 2,
    needs_human_reason: "The migration would drop rows nobody has approved.",
    handoff: null,
    depends_on: [],
    blocks: [],
    created_at: "2026-03-01T09:00:00Z",
    updated_at: "2026-03-01T11:30:00Z",
    closed_at: null,
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

function renderDashboard() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/"]}>
        <DashboardPage />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return client;
}

/** The table under a section heading, once it has rendered. */
async function sectionTable(title: string): Promise<HTMLElement> {
  const heading = await screen.findByRole("heading", { name: title });
  const section = heading.closest("section");
  if (section === null) {
    throw new Error(`no section for ${title}`);
  }
  return within(section).findByRole("table");
}

beforeEach(() => {
  vi.mocked(listSessions).mockResolvedValue([]);
  vi.mocked(listHumanTasks).mockResolvedValue([]);
  vi.mocked(listProjects).mockResolvedValue([]);
  vi.mocked(listUsers).mockResolvedValue([]);
  installSession({ user: user(), access_token: "test-access-token" });
});

afterEach(() => {
  cleanup();
  clearAuth();
  vi.clearAllMocks();
});

describe("DashboardPage", () => {
  it("renders one row per section, linking to the session or task", async () => {
    vi.mocked(listSessions).mockImplementation((params = {}) =>
      Promise.resolve(
        params.state === "running"
          ? [session()]
          : [
              session({
                id: "00000000-0000-0000-0000-000000000a11",
                state: "parked",
                title: null,
                parked_at: "2026-03-01T11:45:00Z",
              }),
            ],
      ),
    );
    vi.mocked(listHumanTasks).mockResolvedValue([task()]);
    vi.mocked(listProjects).mockResolvedValue([project()]);
    vi.mocked(listUsers).mockResolvedValue([user()]);

    renderDashboard();

    const running = await sectionTable("Running sessions");
    expect(
      within(running)
        .getByRole("link", { name: "Wire the dashboard" })
        .getAttribute("href"),
    ).toBe("/sessions/00000000-0000-0000-0000-0000000000d4");
    // The project name, not its id.
    expect(within(running).getByText("mars")).toBeDefined();
    expect(within(running).getByText("$1.50")).toBeDefined();

    // A session with no title says so instead of showing an empty cell.
    const parked = await sectionTable("Parked sessions");
    expect(
      within(parked)
        .getByRole("link", { name: "Untitled session" })
        .getAttribute("href"),
    ).toBe("/sessions/00000000-0000-0000-0000-000000000a11");

    // The link uses the task's own project, which is not the one in the list.
    const human = await sectionTable("Needs a human");
    expect(
      within(human)
        .getByRole("link", { name: "Decide the retention window" })
        .getAttribute("href"),
    ).toBe(`/projects/${OTHER_PROJECT_ID}/tasks/42`);
    // No project row for it: the short id stands in.
    expect(within(human).getByText(OTHER_PROJECT_ID.slice(0, 8))).toBeDefined();
    expect(within(human).getByText("P0")).toBeDefined();
    expect(within(human).getByText("operator")).toBeDefined();
  });

  it("shows an empty state per section when every list is empty", async () => {
    renderDashboard();

    expect(await screen.findByText("No running sessions")).toBeDefined();
    expect(await screen.findByText("No parked sessions")).toBeDefined();
    expect(
      await screen.findByText("Nothing is waiting for a human"),
    ).toBeDefined();
  });

  it("keeps the other sections when one query fails", async () => {
    vi.mocked(listSessions).mockImplementation((params = {}) =>
      params.state === "running"
        ? Promise.reject(new Error("orchestrator unreachable"))
        : Promise.resolve([session({ state: "parked", title: "Parked one" })]),
    );

    renderDashboard();

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Could not load running sessions");
    expect(
      within(alert).getByRole("button", { name: "Try again" }),
    ).toBeDefined();

    const parked = await sectionTable("Parked sessions");
    expect(within(parked).getByRole("link", { name: "Parked one" })).toBeDefined();
  });

  it("does not call a failed section empty", async () => {
    vi.mocked(listSessions).mockImplementation((params = {}) =>
      params.state === "running"
        ? Promise.reject(new Error("orchestrator unreachable"))
        : Promise.resolve([]),
    );

    renderDashboard();

    // The alert is the whole answer: a read that failed knows nothing about
    // how many running sessions there are (`SPEC.md`, "Frontend", Read
    // failures).
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Could not load running sessions");
    expect(screen.queryByText("No running sessions")).toBeNull();
    expect(await screen.findByText("No parked sessions")).toBeDefined();
  });

  it("polls the three lists every 30 seconds, and only in the foreground", async () => {
    renderDashboard();
    await screen.findByText("No running sessions");

    const polled = vi
      .mocked(useQuery)
      .mock.calls.map(([options]) => options)
      .filter(
        (options) =>
          JSON.stringify(options.queryKey).includes("sessions") ||
          JSON.stringify(options.queryKey).includes("tasks"),
      );

    expect(polled.length).toBeGreaterThan(0);
    for (const options of polled) {
      expect(options.refetchInterval).toBe(DASHBOARD_REFETCH_MS);
      expect(options.refetchIntervalInBackground).toBe(false);
    }

    // The projects list rides the default interval.
    const projectsOptions = vi
      .mocked(useQuery)
      .mock.calls.map(([options]) => options)
      .find((options) => JSON.stringify(options.queryKey).includes("projects"));
    expect(projectsOptions?.refetchInterval).toBeUndefined();
  });

  it("does not read the users list for a non-admin", async () => {
    clearAuth();
    installSession({
      user: user({ admin: false }),
      access_token: "test-access-token",
    });

    renderDashboard();
    await screen.findByText("No running sessions");

    await waitFor(() => {
      expect(vi.mocked(listUsers)).not.toHaveBeenCalled();
    });
  });
});
