import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
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
import { ApiError } from "../services/apiClient";
import { listTasks } from "../services/tasks";
import {
  createTaskState,
  deleteTaskState,
  listTaskStates,
  updateTaskState,
} from "../services/taskStates";
import type { Task, TaskState, TaskStateKind } from "../types";
import { TaskStatesEditor } from "./TaskStatesEditor";

vi.mock("../services/tasks", () => ({ listTasks: vi.fn() }));
vi.mock("../services/taskStates", () => ({
  listTaskStates: vi.fn(),
  createTaskState: vi.fn(),
  updateTaskState: vi.fn(),
  deleteTaskState: vi.fn(),
}));

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "00000000-0000-0000-0000-0000000000a1";

function state(name: string, kind: TaskStateKind, position: number): TaskState {
  return {
    id: `00000000-0000-0000-0000-0000000000${(position + 10).toString(16)}`,
    project_id: PROJECT_ID,
    name,
    kind,
    position,
    auto_merge: false,
    conflict_state: null,
    created_at: "2026-03-01T09:00:00Z",
  };
}

function task(stateName: string, number: number): Task {
  return {
    id: `00000000-0000-0000-0000-0000000001${(number + 10).toString(16)}`,
    project_id: PROJECT_ID,
    number,
    title: `Task ${String(number)}`,
    description: null,
    state: stateName,
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
    created_by_session_id: null,
    created_at: "2026-03-01T09:00:00Z",
    updated_at: "2026-03-01T09:00:00Z",
    closed_at: null,
  };
}

const READY = state("ready", "queue", 0);
const REVIEW = state("review", "queue", 1);
const DEFAULT_STATES = [
  READY,
  REVIEW,
  state("needs_human", "human", 2),
  state("done", "terminal", 3),
];

function renderEditor() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      {/* The header's `Learn more` is a router link. */}
      <MemoryRouter>
        <TaskStatesEditor projectId={PROJECT_ID} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The `<tr>` of one state, found by its name cell. */
async function row(name: string): Promise<HTMLElement> {
  const cell = await screen.findByText(name);
  const tr = cell.closest("tr");
  if (tr === null) {
    throw new Error(`no row for ${name}`);
  }
  return tr;
}

beforeEach(() => {
  vi.mocked(listTaskStates).mockResolvedValue(DEFAULT_STATES);
  vi.mocked(listTasks).mockResolvedValue([task("ready", 1), task("ready", 2)]);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("TaskStatesEditor", () => {
  it("lists the states in position order with their kind and task count", async () => {
    renderEditor();

    const names = await screen.findAllByText(
      /^(ready|review|needs_human|done)$/,
    );
    expect(names.map((node) => node.textContent)).toEqual([
      "ready",
      "review",
      "needs_human",
      "done",
    ]);

    expect(within(await row("ready")).getByText("queue")).toBeTruthy();
    expect(within(await row("ready")).getByText("2")).toBeTruthy();
    expect(within(await row("needs_human")).getByText("human")).toBeTruthy();
  });

  it("disables removal with the reason for each of the four refusals", async () => {
    renderEditor();

    // The human state, whatever else is true of it.
    const human = within(await row("needs_human")).getByRole("button", {
      name: "Remove",
    });
    expect(human.hasAttribute("disabled")).toBe(true);
    // Said as text at every width, and read with the button it shuts.
    const humanReason = within(await row("needs_human")).getByText(
      "The human state cannot be deleted",
    );
    expect(humanReason.className).not.toContain("hidden");
    expect(human.getAttribute("aria-describedby")).toBe(humanReason.id);

    // `done` is the only terminal state.
    expect(
      within(await row("done")).getByText(
        "The last terminal state cannot be deleted",
      ),
    ).toBeTruthy();

    // `ready` holds two tasks; `review` holds none and is one of two queues.
    expect(
      within(await row("ready")).getByText("2 tasks are in this state"),
    ).toBeTruthy();
    const review = within(await row("review")).getByRole("button", {
      name: "Remove",
    });
    expect(review.hasAttribute("disabled")).toBe(false);
  });

  it("closes the human option once the project has a human state", async () => {
    renderEditor();

    const option = await screen.findByRole("option", {
      name: /already has a human state/,
    });
    expect(option.hasAttribute("disabled")).toBe(true);
    expect(screen.getByRole("option", { name: "queue" })).toBeTruthy();
  });

  it("refuses an invalid name before the request goes out", async () => {
    renderEditor();
    await screen.findByText("ready");

    fireEvent.change(screen.getByLabelText(/^Name/), {
      target: { value: "Needs Human" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add state" }));

    await screen.findByText("Name must be 1–32 characters of a-z, 0-9, _ or -");
    expect(createTaskState).not.toHaveBeenCalled();
  });

  it("adds a state and shows the server's refusal verbatim", async () => {
    vi.mocked(createTaskState).mockRejectedValueOnce(
      new ApiError(409, "state name is taken"),
    );
    renderEditor();
    await screen.findByText("ready");

    fireEvent.change(screen.getByLabelText(/^Name/), {
      target: { value: "blocked" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add state" }));

    await screen.findByText("state name is taken");
    expect(createTaskState).toHaveBeenCalledWith(PROJECT_ID, {
      name: "blocked",
      kind: "queue",
    });
  });

  it("moves a row by its index and disables the ends", async () => {
    vi.mocked(updateTaskState).mockResolvedValue(REVIEW);
    renderEditor();

    const first = within(await row("ready"));
    expect(
      first
        .getByRole("button", { name: /Move ready up/ })
        .hasAttribute("disabled"),
    ).toBe(true);
    const last = within(await row("done"));
    expect(
      last
        .getByRole("button", { name: /Move done down/ })
        .hasAttribute("disabled"),
    ).toBe(true);

    fireEvent.click(
      within(await row("review")).getByRole("button", {
        name: /Move review up/,
      }),
    );
    await waitFor(() => {
      expect(updateTaskState).toHaveBeenCalledWith(PROJECT_ID, "review", {
        position: 0,
      });
    });
  });

  it("renames a state without ever sending its kind", async () => {
    vi.mocked(updateTaskState).mockResolvedValue(state("triage", "queue", 1));
    renderEditor();

    fireEvent.click(
      within(await row("review")).getByRole("button", { name: "Rename" }),
    );
    fireEvent.change(screen.getByLabelText("New name"), {
      target: { value: "triage" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(updateTaskState).toHaveBeenCalledWith(PROJECT_ID, "review", {
        name: "triage",
      });
    });
  });

  it("takes the rename's refusal with it when the rename is cancelled", async () => {
    vi.mocked(updateTaskState).mockRejectedValueOnce(
      new ApiError(409, "state name is taken"),
    );
    renderEditor();

    fireEvent.click(
      within(await row("review")).getByRole("button", { name: "Rename" }),
    );
    fireEvent.change(screen.getByLabelText("New name"), {
      target: { value: "ready" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await screen.findByText("state name is taken");

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => {
      expect(screen.queryByText("state name is taken")).toBeNull();
    });
  });

  it("lets a row's answer describe its last action, not an older one", async () => {
    vi.mocked(updateTaskState).mockRejectedValueOnce(
      new ApiError(409, "positions changed"),
    );
    vi.mocked(deleteTaskState).mockResolvedValueOnce(undefined);
    renderEditor();

    fireEvent.click(
      within(await row("review")).getByRole("button", {
        name: /Move review up/,
      }),
    );
    await screen.findByText("positions changed");

    // A different action on the same row, and it worked: the row has nothing
    // left to complain about.
    fireEvent.click(
      within(await row("review")).getByRole("button", { name: "Remove" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Remove review" }));
    await waitFor(() => {
      expect(screen.queryByText("positions changed")).toBeNull();
    });
  });

  it("surfaces a delete conflict that arrives anyway", async () => {
    vi.mocked(deleteTaskState).mockRejectedValueOnce(
      new ApiError(409, "state is in use by tasks"),
    );
    renderEditor();

    fireEvent.click(
      within(await row("review")).getByRole("button", { name: "Remove" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Remove review" }));

    await screen.findByText("state is in use by tasks");
    // Both lists are read again, so the row tells the truth next time.
    await waitFor(() => {
      expect(vi.mocked(listTaskStates).mock.calls.length).toBeGreaterThan(1);
      expect(vi.mocked(listTasks).mock.calls.length).toBeGreaterThan(1);
    });
  });

  it("turns auto-merge on with ready pre-selected and saves the pair in one PUT", async () => {
    vi.mocked(updateTaskState).mockResolvedValue({
      ...REVIEW,
      auto_merge: true,
      conflict_state: "ready",
    });
    renderEditor();

    const review = await row("review");
    fireEvent.click(
      within(review).getByRole("checkbox", { name: "Auto-merge" }),
    );

    const select = within(review).getByRole("combobox", {
      name: "Conflict state",
    });
    expect((select as HTMLSelectElement).value).toBe("ready");
    // Only the project's other queue states are offered.
    expect(
      within(select)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual(["ready"]);
    // Nothing is sent until the pair is saved.
    expect(updateTaskState).not.toHaveBeenCalled();

    fireEvent.click(within(review).getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(updateTaskState).toHaveBeenCalledTimes(1);
    });
    expect(updateTaskState).toHaveBeenCalledWith(PROJECT_ID, "review", {
      auto_merge: true,
      conflict_state: "ready",
    });
  });

  it("shows an auto-merge refusal in the row's words", async () => {
    vi.mocked(updateTaskState).mockRejectedValueOnce(
      new ApiError(
        400,
        "conflict_state must name a queue state of this project",
      ),
    );
    renderEditor();

    const review = await row("review");
    fireEvent.click(
      within(review).getByRole("checkbox", { name: "Auto-merge" }),
    );
    fireEvent.click(within(review).getByRole("button", { name: "Save" }));

    await screen.findByText(/must be another queue state of this project/);
  });

  it("disables removing a conflict state, naming the state that uses it", async () => {
    vi.mocked(listTaskStates).mockResolvedValue([
      READY,
      { ...REVIEW, auto_merge: true, conflict_state: "ready" },
      state("backlog", "queue", 2),
      state("needs_human", "human", 3),
      state("done", "terminal", 4),
    ]);
    vi.mocked(listTasks).mockResolvedValue([]);
    renderEditor();

    // The review row's select also lists `ready`, so the row is found by
    // its name cell rather than by the first text that matches.
    const cells = await screen.findAllByText("ready", { selector: "span" });
    const ready = cells[0]?.closest("tr");
    if (ready === null || ready === undefined) {
      throw new Error("no row for ready");
    }
    expect(
      within(ready).getByText("This is the conflict state of review"),
    ).toBeTruthy();
    expect(
      within(ready)
        .getByRole("button", { name: "Remove" })
        .hasAttribute("disabled"),
    ).toBe(true);
  });
});
