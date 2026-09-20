// The matching rules of `SPEC.md`, "Frontend", "Task-board search": exact
// numbers with or without `#`, case-insensitive title substrings, and an empty
// query that filters nothing at all.

import { describe, expect, it } from "vitest";

import type { Task } from "../types";
import { filterTasks, normalizeQuery } from "./search";

const PROJECT = "11111111-1111-4111-8111-111111111111";

function task(number: number, title: string, state = "ready"): Task {
  return {
    id: `task-${number}`,
    project_id: PROJECT,
    number,
    title,
    description: null,
    state,
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
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    closed_at: null,
  };
}

const T42 = task(42, "Fix Login Redirect");
const T142 = task(142, "Rotate the deploy key");
const T420 = task(420, "Archive stale branches");
const DONE = task(7, "Ship the login banner", "done");

const TASKS = [T42, T142, T420, DONE];

function numbers(tasks: Task[]): number[] {
  return tasks.map((task) => task.number);
}

describe("normalizeQuery", () => {
  it("trims the edges and keeps the middle", () => {
    expect(normalizeQuery("  #42 ")).toBe("#42");
    expect(normalizeQuery("\tfix login\n")).toBe("fix login");
    expect(normalizeQuery("   ")).toBe("");
  });
});

describe("filterTasks by number", () => {
  it("matches the exact number, with or without the hash", () => {
    expect(numbers(filterTasks(TASKS, "42"))).toEqual([42]);
    expect(numbers(filterTasks(TASKS, "#42"))).toEqual([42]);
  });

  it("matches through surrounding whitespace", () => {
    expect(numbers(filterTasks(TASKS, " #42 "))).toEqual([42]);
  });

  it("never matches a task the digits are only a prefix of", () => {
    const found = filterTasks(TASKS, "42");
    expect(numbers(found)).not.toContain(142);
    expect(numbers(found)).not.toContain(420);
  });

  it("ignores leading zeros", () => {
    expect(numbers(filterTasks(TASKS, "042"))).toEqual([42]);
    expect(numbers(filterTasks(TASKS, "#0042"))).toEqual([42]);
  });

  it("matches nothing for a digit string no task can carry", () => {
    // Long enough that `Number()` would round it onto another value; compared
    // as a string, it simply matches no task.
    expect(filterTasks(TASKS, "9007199254740993")).toEqual([]);
  });

  it("matches a task in a terminal state like any other", () => {
    expect(numbers(filterTasks(TASKS, "#7"))).toEqual([7]);
  });
});

describe("filterTasks by title", () => {
  it("matches a substring of the title", () => {
    expect(numbers(filterTasks(TASKS, "login"))).toEqual([42, 7]);
  });

  it("ignores case on both sides", () => {
    expect(numbers(filterTasks(TASKS, "LOGIN"))).toEqual([42, 7]);
    expect(numbers(filterTasks(TASKS, "Login"))).toEqual([42, 7]);
  });

  it("matches a task in a terminal state", () => {
    expect(numbers(filterTasks(TASKS, "banner"))).toEqual([7]);
  });

  it("treats a lone hash as a title search", () => {
    expect(filterTasks(TASKS, "#")).toEqual([]);
    const hashed = [...TASKS, task(9, "Handle a # in the title")];
    expect(numbers(filterTasks(hashed, "#"))).toEqual([9]);
  });

  it("never looks at the description", () => {
    const described = [{ ...T142, description: "the login key" }];
    expect(filterTasks(described, "login")).toEqual([]);
  });

  it("preserves the input order", () => {
    const shuffled = [T420, DONE, T42, T142];
    expect(numbers(filterTasks(shuffled, "e"))).toEqual([420, 7, 42, 142]);
  });
});

describe("filterTasks with no query", () => {
  it("returns the same array reference for an empty query", () => {
    expect(filterTasks(TASKS, "")).toBe(TASKS);
  });

  it("returns the same array reference for a whitespace-only query", () => {
    expect(filterTasks(TASKS, "   ")).toBe(TASKS);
  });
});
