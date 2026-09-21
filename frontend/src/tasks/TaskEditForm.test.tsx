// The edit form's baseline (`SPEC.md`, "Tasks": a `PUT` leaves omitted fields
// alone; "Frontend", "Board refresh ordering": the drawer is rereading the
// task while the form is open).
//
// The scenario is the one the diff exists for: the form opens, the user
// changes the title, an SSE event or a board refresh hands the form the same
// task with a different priority and description, and the save must still
// carry the title alone.

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
import { getUser } from "../services/users";
import type { TaskDetail, User } from "../types";
import { TaskEditForm } from "./TaskEditForm";
import { useTaskStore } from "./taskStore";

vi.mock("../services/tasks", () => ({ updateTask: vi.fn() }));
vi.mock("../services/users", () => ({ listUsers: vi.fn(), getUser: vi.fn() }));

const updateTaskMock = vi.mocked(updateTask);
const getUserMock = vi.mocked(getUser);

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "11111111-1111-4111-8111-111111111111";
const TASK_ID = "22222222-2222-4222-8222-222222222222";
const NUMBER = 7;
const ASSIGNEE = "33333333-3333-4333-8333-333333333333";

function detail(overrides: Partial<TaskDetail> = {}): TaskDetail {
  return {
    id: TASK_ID,
    project_id: PROJECT_ID,
    number: NUMBER,
    title: "Original title",
    description: "Original description",
    state: "ready",
    priority: 2,
    blocked: false,
    labels: ["api"],
    parent_id: null,
    assignee_user_id: null,
    lease_holder_session_id: null,
    lease_since: null,
    attempts: 0,
    needs_human_reason: null,
    handoff: null,
    depends_on: [],
    blocks: [],
    created_at: "2026-03-01T09:00:00Z",
    updated_at: "2026-03-01T09:00:00Z",
    closed_at: null,
    comments: [],
    handoffs: [],
    children: [],
    sessions: [],
    ...overrides,
  };
}

function renderForm(task: TaskDetail, onDone = vi.fn()) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const view = render(
    <QueryClientProvider client={client}>
      <TaskEditForm projectId={PROJECT_ID} task={task} onDone={onDone} />
    </QueryClientProvider>,
  );
  return {
    onDone,
    rerender: (next: TaskDetail) => {
      view.rerender(
        <QueryClientProvider client={client}>
          <TaskEditForm projectId={PROJECT_ID} task={next} onDone={onDone} />
        </QueryClientProvider>,
      );
    },
  };
}

function typeTitle(value: string) {
  fireEvent.change(screen.getByLabelText(/^Title/), { target: { value } });
}

function save() {
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
}

/** The task as the server answers it after the concurrent change. */
const moved = detail({
  priority: 0,
  description: "Rewritten by an agent",
  updated_at: "2026-03-01T09:05:00Z",
});

beforeEach(() => {
  updateTaskMock.mockReset();
  updateTaskMock.mockResolvedValue(detail());
  getUserMock.mockReset();
  getUserMock.mockResolvedValue({
    id: ASSIGNEE,
    username: "agent",
    email: "agent@example.invalid",
    admin: false,
    must_change_password: false,
    notify_email: true,
    created_at: "2026-03-01T09:00:00Z",
  } satisfies User);
  useTaskStore.setState({ tasks: [] });
});

afterEach(() => {
  cleanup();
});

describe("TaskEditForm", () => {
  it("sends only the edited title when the server changed other fields", async () => {
    const { rerender } = renderForm(detail());

    typeTitle("A better title");
    rerender(moved);
    save();

    await waitFor(() => {
      expect(updateTaskMock).toHaveBeenCalledTimes(1);
    });
    expect(updateTaskMock).toHaveBeenCalledWith(PROJECT_ID, NUMBER, {
      title: "A better title",
    });
  });

  it("makes no request at all when the concurrent change is the only change", async () => {
    const { rerender, onDone } = renderForm(detail());

    rerender(moved);
    save();

    await waitFor(() => {
      expect(onDone).toHaveBeenCalledTimes(1);
    });
    expect(updateTaskMock).not.toHaveBeenCalled();
  });

  it("says the task changed while the form was open", () => {
    const { rerender } = renderForm(detail());

    expect(screen.queryByText(/changed while you were editing/)).toBeNull();
    rerender(moved);
    expect(screen.getByText(/changed while you were editing/)).not.toBeNull();
  });

  it("still clears the assignee explicitly", async () => {
    const { rerender } = renderForm(detail({ assignee_user_id: ASSIGNEE }));

    fireEvent.change(screen.getByLabelText(/^Assignee/), {
      target: { value: "" },
    });
    rerender(
      detail({
        assignee_user_id: ASSIGNEE,
        priority: 0,
      }),
    );
    save();

    await waitFor(() => {
      expect(updateTaskMock).toHaveBeenCalledTimes(1);
    });
    expect(updateTaskMock).toHaveBeenCalledWith(PROJECT_ID, NUMBER, {
      assignee_user_id: null,
    });
  });

  it("keeps the drafted assignee in the select when the server unassigns", async () => {
    const { rerender } = renderForm(detail({ assignee_user_id: ASSIGNEE }));

    const select = await screen.findByLabelText(/^Assignee/);
    rerender(detail({ assignee_user_id: null }));

    expect((select as HTMLSelectElement).value).toBe(ASSIGNEE);
    expect(
      [...(select as HTMLSelectElement).options].map((option) => option.value),
    ).toContain(ASSIGNEE);
  });

  it("refuses an empty title without sending anything", () => {
    renderForm(detail());

    typeTitle("   ");
    save();

    expect(screen.getByText("A task needs a title")).not.toBeNull();
    expect(updateTaskMock).not.toHaveBeenCalled();
  });
});
