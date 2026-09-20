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
    expect(counts).toEqual({ ready: 2, done: 1 });
  });
});

describe("deletionReason", () => {
  const states = [READY, REVIEW, NEEDS_HUMAN, DONE, ARCHIVED];

  it("refuses the human state", () => {
    expect(deletionReason(NEEDS_HUMAN, states, {})).toBe(
      "The human state cannot be deleted",
    );
  });

  it("refuses the last queue state", () => {
    const one = [READY, NEEDS_HUMAN, DONE];
    expect(deletionReason(READY, one, {})).toBe(
      "The last queue state cannot be deleted",
    );
  });

  it("refuses the last terminal state", () => {
    const one = [READY, NEEDS_HUMAN, DONE];
    expect(deletionReason(DONE, one, {})).toBe(
      "The last terminal state cannot be deleted",
    );
  });

  it("refuses a state that still holds tasks, counting them", () => {
    expect(deletionReason(REVIEW, states, { review: 3 })).toBe(
      "3 tasks are in this state",
    );
    expect(deletionReason(REVIEW, states, { review: 1 })).toBe(
      "1 task is in this state",
    );
  });

  it("allows an empty state that is neither the last queue nor the last terminal", () => {
    expect(deletionReason(REVIEW, states, { ready: 2 })).toBeNull();
    expect(deletionReason(ARCHIVED, states, { done: 5 })).toBeNull();
  });
});
