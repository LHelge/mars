import { describe, expect, it } from "vitest";
import type { Task, TaskState, TaskStateKind } from "../types";
import {
  countTasksByState,
  deletionReason,
  NAME_RULE,
  stateNameError,
} from "./taskStateRules";

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
    created_at: "2026-03-01T09:00:00Z",
    updated_at: "2026-03-01T09:00:00Z",
    closed_at: null,
  };
}

/** The default set of `docs/data-model.md`, `task_states`, plus a spare queue. */
const READY = state("ready", "queue", 0);
const REVIEW = state("review", "queue", 1);
const NEEDS_HUMAN = state("needs_human", "human", 2);
const DONE = state("done", "terminal", 3);
const ARCHIVED = state("archived", "terminal", 4);

describe("stateNameError", () => {
  it("accepts the names SPEC allows", () => {
    for (const name of [
      "a",
      "0",
      "ready",
      "needs_human",
      "in-review",
      "x".repeat(32),
    ]) {
      expect(stateNameError(name)).toBeNull();
    }
  });

  it("rejects an empty name, a bad first character, capitals and overlong names", () => {
    for (const name of [
      "",
      "_ready",
      "-ready",
      "Ready",
      "needs human",
      "x".repeat(33),
    ]) {
      expect(stateNameError(name)).toBe(NAME_RULE);
    }
  });
});

describe("countTasksByState", () => {
  it("groups by the state's name and leaves empty states out", () => {
    const counts = countTasksByState([
      task("ready", 1),
      task("ready", 2),
      task("done", 3),
    ]);
    expect(counts).toEqual(
      new Map([
        ["ready", 2],
        ["done", 1],
      ]),
    );
  });

  it("counts a state named after an Object prototype member", () => {
    // `constructor` matches the name rule of `SPEC.md`, "Task states", and a
    // project may well have a state called that. Off a plain object the lookup
    // answers `Object`, the count becomes a string and every comparison on it
    // is `NaN` — so the state reads as empty and its delete button opens.
    const counts = countTasksByState([
      task("constructor", 1),
      task("constructor", 2),
      task("toString", 3),
    ]);

    expect(counts.get("constructor")).toBe(2);
    expect(counts.get("toString")).toBe(1);
    expect(counts.get("valueOf")).toBeUndefined();
  });

  it("refuses to delete a state named `constructor` that holds tasks", () => {
    const CONSTRUCTOR_STATE: TaskState = {
      ...REVIEW,
      id: "00000000-0000-0000-0000-0000000000c0",
      name: "constructor",
    };
    const counts = countTasksByState([task("constructor", 1)]);

    expect(
      deletionReason(
        CONSTRUCTOR_STATE,
        [READY, CONSTRUCTOR_STATE, NEEDS_HUMAN, DONE, ARCHIVED],
        counts,
      ),
    ).toBe("1 task is in this state");
  });
});

describe("deletionReason", () => {
  const states = [READY, REVIEW, NEEDS_HUMAN, DONE, ARCHIVED];

  it("refuses the human state", () => {
    expect(deletionReason(NEEDS_HUMAN, states, new Map())).toBe(
      "The human state cannot be deleted",
    );
  });

  it("refuses the last queue state", () => {
    const one = [READY, NEEDS_HUMAN, DONE];
    expect(deletionReason(READY, one, new Map())).toBe(
      "The last queue state cannot be deleted",
    );
  });

  it("refuses the last terminal state", () => {
    const one = [READY, NEEDS_HUMAN, DONE];
    expect(deletionReason(DONE, one, new Map())).toBe(
      "The last terminal state cannot be deleted",
    );
  });

  it("refuses a state that still holds tasks, counting them", () => {
    expect(deletionReason(REVIEW, states, new Map([["review", 3]]))).toBe(
      "3 tasks are in this state",
    );
    expect(deletionReason(REVIEW, states, new Map([["review", 1]]))).toBe(
      "1 task is in this state",
    );
  });

  it("refuses a state another state names as its conflict state, naming that state", () => {
    const merge: TaskState = {
      ...state("merge", "queue", 5),
      auto_merge: true,
      conflict_state: "review",
    };
    const withMerge = [...states, merge];

    expect(deletionReason(REVIEW, withMerge, new Map())).toBe(
      "This is the conflict state of merge",
    );
    // The auto-merge state itself is not held back by its own pairing.
    expect(deletionReason(merge, withMerge, new Map())).toBeNull();
  });

  it("allows an empty state that is neither the last queue nor the last terminal", () => {
    expect(deletionReason(REVIEW, states, new Map([["ready", 2]]))).toBeNull();
    expect(deletionReason(ARCHIVED, states, new Map([["done", 5]]))).toBeNull();
  });
});
