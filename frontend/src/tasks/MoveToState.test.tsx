// The state a move targets (`SPEC.md`, "Tasks": moving to a different state
// clears the lease and resets `attempts`; "Frontend", "Board refresh
// ordering": the drawer rereads the task while its controls are open).
//
// The scenario is the one the `null` choice exists for: the drawer is open on
// a task in `ready`, an agent hands it to `review`, and the reread arrives at
// a control nobody has touched. Nothing was chosen, so there is nothing to
// send and Move stays disabled — a target seeded from the old state would be
// armed to send the task back.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { updateTask } from "../services/tasks";
import type { Task, TaskState } from "../types";
import { MoveToState } from "./MoveToState";
import { useTaskStore } from "./taskStore";

vi.mock("../services/tasks", () => ({ updateTask: vi.fn() }));

const updateTaskMock = vi.mocked(updateTask);

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "11111111-1111-4111-8111-111111111111";
const TASK_ID = "22222222-2222-4222-8222-222222222222";
const NUMBER = 7;

function task(state: string): Task {
  return {
    id: TASK_ID,
    project_id: PROJECT_ID,
    number: NUMBER,
    title: "A task on the board",
    description: "",
    state,
    priority: 2,
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
    created_at: "2026-03-01T09:00:00Z",
    updated_at: "2026-03-01T09:00:00Z",
    closed_at: null,
  };
}

const STATES: TaskState[] = ["ready", "review", "done"].map(
  (name, index): TaskState => ({
    id: `${String(index + 3).repeat(8)}-3333-4333-8333-333333333333`,
    project_id: PROJECT_ID,
    name,
    kind: name === "done" ? "terminal" : "queue",
    position: index + 1,
    auto_merge: false,
    conflict_state: null,
    created_at: "2026-03-01T09:00:00Z",
  }),
);

function renderControl() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const view = (current: Task) => (
    <QueryClientProvider client={client}>
      <MoveToState projectId={PROJECT_ID} task={current} />
    </QueryClientProvider>
  );
  const rendered = render(view(task("ready")));
  return {
    rerender: (current: Task) => {
      rendered.rerender(view(current));
    },
  };
}

function moveButton(): HTMLButtonElement {
  return screen.getByRole<HTMLButtonElement>("button", { name: "Move" });
}

function select(): HTMLSelectElement {
  return screen.getByLabelText<HTMLSelectElement>("Move to");
}

beforeEach(() => {
  updateTaskMock.mockReset();
  updateTaskMock.mockResolvedValue(task("done"));
  useTaskStore.setState({ states: STATES });
});

afterEach(() => {
  cleanup();
});

describe("MoveToState", () => {
  it("stays disarmed when the task's state changes under an untouched control", () => {
    const { rerender } = renderControl();

    expect(moveButton().disabled).toBe(true);

    rerender(task("review"));

    expect(moveButton().disabled).toBe(true);
    expect(select().value).toBe("review");
  });

  it("keeps the user's choice when the task's state changes", () => {
    const { rerender } = renderControl();

    fireEvent.change(select(), { target: { value: "done" } });
    rerender(task("review"));

    expect(select().value).toBe("done");
    expect(moveButton().disabled).toBe(false);
  });

  it("disarms again when the task reaches the chosen state on its own", () => {
    const { rerender } = renderControl();

    fireEvent.change(select(), { target: { value: "review" } });
    rerender(task("review"));

    expect(moveButton().disabled).toBe(true);
  });

  it("sends the chosen state once confirmed and then follows the task again", async () => {
    const { rerender } = renderControl();

    fireEvent.change(select(), { target: { value: "done" } });
    fireEvent.click(moveButton());
    fireEvent.click(screen.getByRole("button", { name: "Move to done" }));

    await waitFor(() => {
      expect(updateTaskMock).toHaveBeenCalledTimes(1);
    });
    expect(updateTaskMock).toHaveBeenCalledWith(PROJECT_ID, NUMBER, {
      state: "done",
    });

    rerender(task("done"));
    await waitFor(() => {
      expect(moveButton().disabled).toBe(true);
    });
    expect(select().value).toBe("done");
  });
});
