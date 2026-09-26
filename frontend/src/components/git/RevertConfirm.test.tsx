// What a revert sends and how its one refusal that is not the user's reads:
// `expected_head` is the head the table was read at, `reopen` leaves the
// browser only when it was ticked and carries a comment, and a 409 `branch
// has moved` is advice to reload rather than a failure.

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
import { revert } from "../../services/git";
import { listTaskStates } from "../../services/taskStates";
import type { HistoryEntry, TaskState } from "../../types";
import { RevertConfirm } from "./RevertConfirm";

vi.mock("../../services/git", async () => {
  const actual =
    await vi.importActual<typeof import("../../services/git")>(
      "../../services/git",
    );
  return { ...actual, revert: vi.fn() };
});
vi.mock("../../services/taskStates", () => ({ listTaskStates: vi.fn() }));

const revertMock = vi.mocked(revert);
const statesMock = vi.mocked(listTaskStates);

// Obviously fake fixture values (CLAUDE.md, rule 3).
const PROJECT_ID = "00000000-0000-4000-8000-0000000000b2";
const HEAD = "c".repeat(40);
const MIDDLE = "b".repeat(40);
const TARGET = "a".repeat(40);
const REVERT_COMMIT = "d".repeat(40);

function entry(commit: string, taskNumber?: number): HistoryEntry {
  return {
    commit,
    parents: [],
    tree: `${commit.slice(0, 1)}-tree`,
    subject: `subject ${commit.slice(0, 1)}`,
    author_name: "Mars",
    committed_at: "2026-09-25T12:00:00Z",
    requested_by: null,
    reverted_by: null,
    tasks:
      taskNumber === undefined
        ? []
        : [
            {
              id: `00000000-0000-4000-8000-00000000000${String(taskNumber)}`,
              number: taskNumber,
              title: `Task ${String(taskNumber)}`,
              handoff_id: "00000000-0000-4000-8000-0000000000f1",
            },
          ],
    sessions: [],
  };
}

function state(name: string, kind: TaskState["kind"]): TaskState {
  return {
    id: name,
    project_id: PROJECT_ID,
    name,
    kind,
    position: 0,
    auto_merge: false,
    conflict_state: null,
    created_at: "2026-09-25T12:00:00Z",
  };
}

function mount() {
  const onReverted = vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <RevertConfirm
          projectId={PROJECT_ID}
          branch="main"
          target={entry(TARGET)}
          range={[entry(HEAD, 2), entry(MIDDLE, 1)]}
          expectedHead={HEAD}
          formId="revert-test"
          onBusy={vi.fn()}
          onCancel={vi.fn()}
          onReverted={onReverted}
        />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return onReverted;
}

function confirm() {
  fireEvent.click(
    screen.getByRole("button", { name: "Revert main to aaaaaaa" }),
  );
}

beforeEach(() => {
  revertMock.mockReset();
  revertMock.mockResolvedValue({
    commit: REVERT_COMMIT,
    reverted: [],
    reopened: [],
  });
  statesMock.mockReset();
  statesMock.mockResolvedValue([
    state("backlog", "queue"),
    state("ready", "queue"),
    state("needs_human", "human"),
    state("done", "terminal"),
  ]);
});

afterEach(cleanup);

describe("RevertConfirm", () => {
  it("lists what is undone and sends the head it was read at, without reopen", async () => {
    const onReverted = mount();
    expect(
      screen.getByText(/2 commits and these tasks are undone/),
    ).toBeTruthy();
    expect(screen.getByRole("link", { name: "#1 Task 1" })).toBeTruthy();
    expect(screen.getByRole("link", { name: "#2 Task 2" })).toBeTruthy();

    confirm();

    await waitFor(() => {
      expect(onReverted).toHaveBeenCalled();
    });
    expect(revertMock).toHaveBeenCalledWith(PROJECT_ID, {
      branch: "main",
      to: TARGET,
      expected_head: HEAD,
      reopen: undefined,
    });
  });

  it("offers only queue and human states and refuses an empty comment", async () => {
    mount();
    fireEvent.click(screen.getByLabelText("Reopen these tasks"));

    const select = await screen.findByLabelText("Move them to");
    await waitFor(() => {
      expect(
        Array.from((select as HTMLSelectElement).options).map((o) => o.value),
      ).toEqual(["backlog", "ready", "needs_human"]);
    });

    confirm();
    expect(
      await screen.findByText("Say why these tasks are reopened."),
    ).toBeTruthy();
    expect(revertMock).not.toHaveBeenCalled();

    fireEvent.change(select, { target: { value: "ready" } });
    fireEvent.change(screen.getByLabelText("Comment"), {
      target: { value: "  built on the wrong base  " },
    });
    confirm();

    await waitFor(() => {
      expect(revertMock).toHaveBeenCalledWith(PROJECT_ID, {
        branch: "main",
        to: TARGET,
        expected_head: HEAD,
        reopen: { state: "ready", comment: "built on the wrong base" },
      });
    });
  });

  it("reads a moved branch as advice to reload, not as a failure", async () => {
    revertMock.mockRejectedValue(new ApiError(409, "branch has moved"));
    const onReverted = mount();
    confirm();

    expect(
      await screen.findByRole("button", { name: "Reload history" }),
    ).toBeTruthy();
    expect(
      screen.getByText(/The branch has moved since this history was read/),
    ).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(onReverted).not.toHaveBeenCalled();
  });

  it("reads a revert that would change nothing as advice too", async () => {
    revertMock.mockRejectedValue(
      new ApiError(
        409,
        `nothing to revert: main already matches ${TARGET.slice(0, 12)}`,
      ),
    );
    const onReverted = mount();
    confirm();

    expect(
      await screen.findByRole("button", { name: "Reload history" }),
    ).toBeTruthy();
    expect(
      screen.getByText(/The branch already has this commit's content/),
    ).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(onReverted).not.toHaveBeenCalled();
  });

  it("shows any other refusal in the server's words", async () => {
    revertMock.mockRejectedValue(
      new ApiError(400, "reopen state must be a queue or human state"),
    );
    mount();
    confirm();

    expect((await screen.findByRole("alert")).textContent).toContain(
      "reopen state must be a queue or human state",
    );
  });
});
