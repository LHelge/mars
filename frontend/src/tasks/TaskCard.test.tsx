// A card's chips are named by what they mean (`SPEC.md`, "Frontend", "Mobile
// layout": a `title` is never the only place a fact lives). The short text is
// for the eye and the board's legend explains it; the accessible name is what
// a screen reader, and a phone with no hover, gets.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { getUser } from "../services/users";
import type { Task } from "../types";
import { TaskCard } from "./TaskCard";
import { emptyTaskBoardState, taskSnapshot, useTaskStore } from "./taskStore";

vi.mock("../services/users", () => ({ getUser: vi.fn() }));

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "00000000-0000-0000-0000-0000000000b2";
const PARENT_ID = "task-parent";

function task(overrides: Partial<Task>): Task {
  return {
    id: "task-7",
    project_id: PROJECT_ID,
    number: 7,
    title: "Wire the legend",
    description: null,
    state: "ready",
    priority: 1,
    blocked: false,
    labels: [],
    parent_id: null,
    assignee_user_id: null,
    lease_holder_session_id: null,
    lease_since: null,
    attempts: 0,
    rounds: 0,
    needs_human_reason: null,
    handoff: null,
    depends_on: [],
    blocks: [],
    created_by_session_id: null,
    created_at: "2026-03-01T09:00:00Z",
    updated_at: "2026-03-01T09:00:00Z",
    closed_at: null,
    ...overrides,
  };
}

function show(card: Task, maxRounds?: number): void {
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter>
        <TaskCard task={card} maxRounds={maxRounds} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.mocked(getUser).mockResolvedValue({
    id: "user-1",
    username: "someone",
    email: "someone@example.test",
    admin: false,
    must_change_password: false,
    notify_email: true,
    created_at: "2026-03-01T09:00:00Z",
  });
  useTaskStore.setState({
    ...emptyTaskBoardState(),
    ...taskSnapshot(
      [],
      [task({ id: PARENT_ID, number: 3, title: "The parent" })],
    ),
    projectId: PROJECT_ID,
    loaded: true,
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  useTaskStore.setState(emptyTaskBoardState());
});

describe("TaskCard chips", () => {
  it("names every chip by its meaning", async () => {
    show(
      task({
        priority: 1,
        parent_id: PARENT_ID,
        blocked: true,
        depends_on: [
          { task_id: "a", kind: "blocks" },
          { task_id: "b", kind: "blocks" },
        ],
        blocks: ["c"],
        attempts: 3,
        rounds: 2,
        assignee_user_id: "user-1",
        labels: ["ui"],
      }),
      5,
    );

    const name = (label: string) => screen.getByRole("img", { name: label });

    expect(name("Priority P1 — high").textContent).toBe("P1");
    expect(name("Child of task #3").textContent).toBe("part of #3");
    expect(
      name("Blocked: waiting on open children or unsatisfied dependencies")
        .textContent,
    ).toBe("blocked");
    expect(name("Blocked by 2 tasks; blocks 1 task").textContent).toContain(
      "↑2",
    );
    expect(name("3 sessions picked this task up").textContent).toBe(
      "3 attempts",
    );
    expect(name("Revision round 2 of 5").textContent).toBe("round 2/5");
    expect(name("Label ui").textContent).toBe("ui");
    expect(
      (await screen.findByRole("img", { name: "Assigned to someone" }))
        .textContent,
    ).toBe("@someone");
  });

  it("names a one-way dependency chip by the one direction it has", () => {
    show(task({ blocks: ["c", "d"] }));

    expect(
      screen.getByRole("img", { name: "Blocks 2 tasks" }).textContent,
    ).toBe("↓2");
  });
});
