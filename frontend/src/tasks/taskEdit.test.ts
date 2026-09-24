// What a save carries and what the parent select may offer (`SPEC.md`,
// "Tasks": a `PUT` leaves omitted fields alone, `null` clears the assignee and
// the parent, and nesting is one level deep).

import { describe, expect, it } from "vitest";

import type { Task } from "../types";
import {
  diffTaskInput,
  isEmptyUpdate,
  parentCandidates,
  taskEditValues,
  taskEditValuesDiffer,
} from "./taskEdit";

const PROJECT = "11111111-1111-4111-8111-111111111111";
const ALICE = "22222222-2222-4222-8222-222222222222";
const BOB = "33333333-3333-4333-8333-333333333333";

function task(number: number, overrides: Partial<Task> = {}): Task {
  return {
    id: `task-${String(number)}`,
    project_id: PROJECT,
    number,
    title: `Task ${String(number)}`,
    description: null,
    state: "ready",
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
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    closed_at: null,
    ...overrides,
  };
}

describe("diffTaskInput", () => {
  const original = taskEditValues(
    task(7, {
      title: "Rewrite the importer",
      description: "As it stands it retries forever.",
      priority: 1,
      labels: ["backend", "migration"],
      assignee_user_id: ALICE,
      parent_id: "task-1",
    }),
  );

  it("sends nothing when nothing was touched", () => {
    const input = diffTaskInput(original, { ...original });
    expect(input).toEqual({});
    expect(isEmptyUpdate(input)).toBe(true);
  });

  it("sends only the fields that changed", () => {
    expect(diffTaskInput(original, { ...original, priority: 0 })).toEqual({
      priority: 0,
    });
  });

  it("trims the title, and treats trailing whitespace as no change", () => {
    expect(
      diffTaskInput(original, {
        ...original,
        title: "  Rewrite the importer ",
      }),
    ).toEqual({});
    expect(
      diffTaskInput(original, { ...original, title: "  Rewrite the reader " }),
    ).toEqual({ title: "Rewrite the reader" });
  });

  it("clears the assignee and the parent with an explicit null", () => {
    expect(
      diffTaskInput(original, {
        ...original,
        assignee_user_id: null,
        parent_id: null,
      }),
    ).toEqual({ assignee_user_id: null, parent_id: null });
  });

  it("sends a reassignment as the new id", () => {
    expect(
      diffTaskInput(original, { ...original, assignee_user_id: BOB }),
    ).toEqual({ assignee_user_id: BOB });
  });

  it("ignores label order, since the API stores a set", () => {
    expect(
      diffTaskInput(original, {
        ...original,
        labels: ["migration", "backend"],
      }),
    ).toEqual({});
    expect(
      diffTaskInput(original, { ...original, labels: ["backend"] }),
    ).toEqual({ labels: ["backend"] });
  });

  it("sends an emptied description as an empty string", () => {
    expect(diffTaskInput(original, { ...original, description: "" })).toEqual({
      description: "",
    });
  });

  it("reads a task with no description as an empty field", () => {
    expect(taskEditValues(task(9)).description).toBe("");
  });
});

describe("parentCandidates", () => {
  const parent = task(1);
  const child = task(2, { parent_id: "task-1" });
  const loose = task(3);
  const snapshot = [parent, child, loose];

  it("excludes the task itself", () => {
    expect(parentCandidates(snapshot, loose).map((one) => one.id)).toEqual([
      "task-1",
    ]);
  });

  it("excludes tasks that already have a parent", () => {
    expect(parentCandidates(snapshot, parent).map((one) => one.id)).toEqual([
      "task-3",
    ]);
  });

  it("keeps a task that has children: it is a valid parent", () => {
    expect(parentCandidates(snapshot, loose)).toContain(parent);
  });
});

describe("taskEditValuesDiffer", () => {
  const values = taskEditValues(task(1, { labels: ["api", "ui"] }));

  it("is false for the same reading twice", () => {
    expect(taskEditValuesDiffer(values, { ...values })).toBe(false);
  });

  it("is false when only the label order moved", () => {
    expect(
      taskEditValuesDiffer(values, { ...values, labels: ["ui", "api"] }),
    ).toBe(false);
  });

  it("is true when the server changed a field", () => {
    expect(taskEditValuesDiffer(values, { ...values, priority: 0 })).toBe(true);
  });

  it("is true when the server cleared the assignee", () => {
    const held = { ...values, assignee_user_id: ALICE };
    expect(taskEditValuesDiffer(held, values)).toBe(true);
  });
});
