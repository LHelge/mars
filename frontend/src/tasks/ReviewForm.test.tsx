// The hand-off a review is about (`SPEC.md`, "Frontend", "Hand-off controls":
// review actions forward the hand-off the form opened on; "Tasks": forwarding
// a hand-off that is no longer current is a 409).
//
// The scenario is the one the pin exists for: a reviewer opens Approve on
// commit A and writes a comment, an agent publishes revision B, the drawer
// rereads the task and hands the open form the new current hand-off. The
// submitted id must still be A's — that is what lets the server refuse it —
// and the form must say that B arrived instead of quietly approving it.

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
import type { Handoff, TaskDetail, TaskState } from "../types";
import { ReviewForm } from "./ReviewForm";
import { useTaskStore } from "./taskStore";

vi.mock("../services/tasks", () => ({ updateTask: vi.fn() }));

const updateTaskMock = vi.mocked(updateTask);

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "11111111-1111-4111-8111-111111111111";
const TASK_ID = "22222222-2222-4222-8222-222222222222";
const NUMBER = 7;
const HANDOFF_A = "44444444-4444-4444-8444-444444444444";
const HANDOFF_B = "55555555-5555-4555-8555-555555555555";
const COMMIT_A = "a".repeat(40);
const COMMIT_B = "b".repeat(40);

function handoff(id: string, commit: string): Handoff {
  return {
    id,
    task_id: TASK_ID,
    source_session_id: null,
    source_branch: "session/one",
    commit,
    comment_id: null,
    review_status: "unreviewed",
    reviewed_by_user_id: null,
    reviewed_by_session_id: null,
    reviewed_at: null,
    created_by_user_id: null,
    created_by_session_id: null,
    created_at: "2026-03-01T09:00:00Z",
  };
}

function detail(current: Handoff): TaskDetail {
  return {
    id: TASK_ID,
    project_id: PROJECT_ID,
    number: NUMBER,
    title: "A task under review",
    description: "",
    state: "review",
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
    handoff: current,
    depends_on: [],
    blocks: [],
    created_by_session_id: null,
    created_at: "2026-03-01T09:00:00Z",
    updated_at: "2026-03-01T09:00:00Z",
    closed_at: null,
    comments: [],
    handoffs: [current],
    children: [],
    sessions: [],
  };
}

const STATES: TaskState[] = [
  {
    id: "66666666-6666-4666-8666-666666666666",
    project_id: PROJECT_ID,
    name: "ready",
    kind: "queue",
    position: 1,
    auto_merge: false,
    conflict_state: null,
    created_at: "2026-03-01T09:00:00Z",
  },
  {
    id: "77777777-7777-4777-8777-777777777777",
    project_id: PROJECT_ID,
    name: "review",
    kind: "queue",
    position: 2,
    auto_merge: false,
    conflict_state: null,
    created_at: "2026-03-01T09:00:00Z",
  },
];

const FIRST = handoff(HANDOFF_A, COMMIT_A);
const NEWER = handoff(HANDOFF_B, COMMIT_B);

function renderForm() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const onDone = vi.fn();
  const view = (current: Handoff) => (
    <QueryClientProvider client={client}>
      <ReviewForm
        projectId={PROJECT_ID}
        task={detail(current)}
        handoff={current}
        decision="approved"
        onDone={onDone}
      />
    </QueryClientProvider>
  );
  const rendered = render(view(FIRST));
  return {
    onDone,
    rerender: (current: Handoff) => {
      rendered.rerender(view(current));
    },
  };
}

function fillIn() {
  fireEvent.change(screen.getByLabelText(/^Move to/), {
    target: { value: "ready" },
  });
  fireEvent.change(screen.getByLabelText(/^Comment/), {
    target: { value: "Read it line by line" },
  });
}

beforeEach(() => {
  updateTaskMock.mockReset();
  updateTaskMock.mockResolvedValue(detail(FIRST));
  useTaskStore.setState({ states: STATES });
});

afterEach(() => {
  cleanup();
});

describe("ReviewForm", () => {
  it("approves the hand-off it opened on after a newer revision arrives", async () => {
    const { rerender } = renderForm();

    fillIn();
    rerender(NEWER);
    fireEvent.click(screen.getByRole("button", { name: "Approve" }));

    await waitFor(() => {
      expect(updateTaskMock).toHaveBeenCalledTimes(1);
    });
    expect(updateTaskMock).toHaveBeenCalledWith(PROJECT_ID, NUMBER, {
      state: "ready",
      handoff: {
        kind: "forward",
        handoff_id: HANDOFF_A,
        comment: "Read it line by line",
        review: "approved",
      },
    });
  });

  it("keeps naming the commit it opened on and says a newer one arrived", () => {
    const { rerender } = renderForm();

    expect(screen.getByText(new RegExp(COMMIT_A.slice(0, 7)))).not.toBeNull();
    expect(screen.queryByText(/newer revision/)).toBeNull();

    rerender(NEWER);

    expect(screen.getByText(/newer revision/)).not.toBeNull();
    expect(screen.getByText(new RegExp(COMMIT_A.slice(0, 7)))).not.toBeNull();
    expect(screen.queryByText(new RegExp(COMMIT_B.slice(0, 7)))).toBeNull();
  });
});
