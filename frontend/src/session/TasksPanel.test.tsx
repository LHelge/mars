// What the panel reads, and what it does not (`SPEC.md`, "Frontend", "Session
// side panel").
//
// Three rules, all of which used to be broken: the launched task is listed
// once, not once per reading of it; a session that was launched for no task
// asks for no task; and a `session` frame invalidates rather than refetches,
// which is the difference between "refresh what is on screen" and "fire every
// query in this component, disabled ones included".

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { listSessionTasks } from "../services/sessions";
import { getTask } from "../services/tasks";
import type { Session, Task } from "../types";
import { disposeSessionStore, getSessionStore } from "./sessionStore";
import { TasksPanel } from "./TasksPanel";

vi.mock("../services/sessions", () => ({ listSessionTasks: vi.fn() }));
vi.mock("../services/tasks", () => ({ getTask: vi.fn() }));

// Obviously fake fixture values (CLAUDE.md, rule 3).
const SESSION_ID = "00000000-0000-4000-8000-0000000000a1";
const PROJECT_ID = "00000000-0000-4000-8000-0000000000b2";
const LAUNCHED_ID = "00000000-0000-4000-8000-0000000000c3";
const OTHER_ID = "00000000-0000-4000-8000-0000000000d4";

function task(id: string, number: number, title: string): Task {
  return {
    id,
    project_id: PROJECT_ID,
    number,
    title,
    description: null,
    state: "in_progress",
    priority: 2,
    blocked: false,
    labels: [],
    parent_id: null,
    assignee_user_id: null,
    lease_holder_session_id: id === LAUNCHED_ID ? SESSION_ID : null,
    lease_since: null,
    attempts: 1,
    rounds: 0,
    needs_human_reason: null,
    handoff: null,
    depends_on: [],
    blocks: [],
    created_at: "2026-03-01T11:00:00Z",
    updated_at: "2026-03-01T12:00:00Z",
    closed_at: null,
  };
}

const LAUNCHED = task(LAUNCHED_ID, 7, "Fix the login redirect");
const OTHER = task(OTHER_ID, 8, "Tidy the log lines");

function session(taskId: string | null): Session {
  return {
    id: SESSION_ID,
    project_id: PROJECT_ID,
    state: "running",
    task_id: taskId,
  } as Session;
}

function mount(taskId: string | null) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const invalidate = vi.spyOn(client, "invalidateQueries");
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <TasksPanel session={session(taskId)} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { invalidate };
}

/** A `session` frame, which is the panel's one change notification. */
function sessionFrame(state: Session["state"]) {
  act(() => {
    getSessionStore(SESSION_ID)
      .getState()
      .setSession({ ...session(LAUNCHED_ID), state });
  });
}

beforeEach(() => {
  vi.mocked(listSessionTasks).mockResolvedValue([LAUNCHED, OTHER]);
  vi.mocked(getTask).mockResolvedValue({
    ...LAUNCHED,
    comments: [],
    handoffs: [],
    children: [],
    sessions: [],
  });
});

afterEach(() => {
  cleanup();
  disposeSessionStore(SESSION_ID);
  vi.clearAllMocks();
});

describe("TasksPanel", () => {
  it("lists the launched task once and asks for no detail of it", async () => {
    mount(LAUNCHED_ID);

    await waitFor(() => {
      expect(screen.getByText("Tidy the log lines")).toBeDefined();
    });
    // The list already carries the task the session was launched for, so it is
    // shown from there — under `Launched for` and nowhere else.
    expect(screen.getAllByText("Fix the login redirect")).toHaveLength(1);
    expect(screen.getByText("held by this session")).toBeDefined();
    expect(vi.mocked(getTask)).not.toHaveBeenCalled();
  });

  it("says when there is nothing beyond the launched task", async () => {
    vi.mocked(listSessionTasks).mockResolvedValue([LAUNCHED]);
    mount(LAUNCHED_ID);

    await waitFor(() => {
      expect(screen.getByText("Nothing beyond the task above.")).toBeDefined();
    });
  });

  it("reads the task itself when the list does not carry it", async () => {
    vi.mocked(listSessionTasks).mockResolvedValue([OTHER]);
    mount(LAUNCHED_ID);

    await waitFor(() => {
      expect(screen.getByText("Fix the login redirect")).toBeDefined();
    });
    expect(vi.mocked(getTask)).toHaveBeenCalledWith(PROJECT_ID, LAUNCHED_ID);
  });

  it("asks for no task at all when the session was launched for none", async () => {
    vi.mocked(listSessionTasks).mockResolvedValue([]);
    mount(null);

    await waitFor(() => {
      expect(screen.getByText("No tasks")).toBeDefined();
    });
    sessionFrame("parked");

    // `refetch` would have ignored `enabled` and asked for the empty task id.
    await waitFor(() => {
      expect(vi.mocked(getTask)).not.toHaveBeenCalled();
    });
  });

  it("invalidates its queries on a session frame, and not before", async () => {
    const { invalidate } = mount(LAUNCHED_ID);

    await waitFor(() => {
      expect(screen.getByText("Tidy the log lines")).toBeDefined();
    });
    expect(invalidate).not.toHaveBeenCalled();

    sessionFrame("parked");

    await waitFor(() => {
      expect(invalidate).toHaveBeenCalledWith({
        queryKey: ["sessions", SESSION_ID, "tasks"],
      });
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: ["tasks", PROJECT_ID, LAUNCHED_ID],
    });
  });
});
