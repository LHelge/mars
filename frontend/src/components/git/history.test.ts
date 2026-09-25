import { describe, expect, it } from "vitest";

import type { HistoryEntry, HistoryTask, TaskState } from "../../types";
import {
  HISTORY_PAGE_SIZE,
  nextHistoryCursor,
  parseRequestedBy,
  rangeTasks,
  reopenStates,
  revertRange,
} from "./history";

function task(number: number): HistoryTask {
  return {
    id: `task-${String(number)}`,
    number,
    title: `Task ${String(number)}`,
    handoff_id: `handoff-${String(number)}`,
  };
}

function entry(commit: string, tasks: HistoryTask[] = []): HistoryEntry {
  return {
    commit,
    parents: [],
    subject: `commit ${commit}`,
    author_name: "Mars",
    committed_at: "2026-09-25T12:00:00Z",
    requested_by: null,
    tasks,
    sessions: [],
  };
}

function state(name: string, kind: TaskState["kind"]): TaskState {
  return {
    id: name,
    project_id: "p",
    name,
    kind,
    position: 0,
    auto_merge: false,
    conflict_state: null,
    created_at: "2026-09-25T12:00:00Z",
  };
}

describe("parseRequestedBy", () => {
  it("reads the three documented spellings", () => {
    expect(parseRequestedBy("user:u1")).toEqual({ kind: "user", id: "u1" });
    expect(parseRequestedBy("session:s1")).toEqual({
      kind: "session",
      id: "s1",
    });
    expect(parseRequestedBy("system")).toEqual({ kind: "system" });
  });

  it("is null without a trailer", () => {
    expect(parseRequestedBy(null)).toBeNull();
    expect(parseRequestedBy("")).toBeNull();
  });

  it("keeps anything else as written", () => {
    expect(parseRequestedBy("robot:r1")).toEqual({
      kind: "other",
      text: "robot:r1",
    });
    expect(parseRequestedBy("user:")).toEqual({ kind: "other", text: "user:" });
  });
});

describe("revertRange", () => {
  const entries = [entry("c"), entry("b"), entry("a")];

  it("is every entry above the target, newest first", () => {
    expect(revertRange(entries, "a")?.map((e) => e.commit)).toEqual(["c", "b"]);
    expect(revertRange(entries, "b")?.map((e) => e.commit)).toEqual(["c"]);
  });

  it("is null for the head and for a commit not loaded", () => {
    expect(revertRange(entries, "c")).toBeNull();
    expect(revertRange(entries, "z")).toBeNull();
  });
});

describe("rangeTasks", () => {
  it("names each task once, in the order the range first names it", () => {
    const range = [
      entry("c", [task(2)]),
      entry("b", [task(1), task(2)]),
      entry("a"),
    ];
    expect(rangeTasks(range).map((t) => t.number)).toEqual([2, 1]);
  });
});

describe("nextHistoryCursor", () => {
  it("is the last commit of a full page and nothing after a short one", () => {
    const full = Array.from({ length: HISTORY_PAGE_SIZE }, (_, i) =>
      entry(`c${String(i)}`),
    );
    expect(nextHistoryCursor(full)).toBe(`c${String(HISTORY_PAGE_SIZE - 1)}`);
    expect(nextHistoryCursor(full.slice(1))).toBeUndefined();
    expect(nextHistoryCursor([])).toBeUndefined();
  });
});

describe("reopenStates", () => {
  it("keeps queue and human states in board order and drops terminal ones", () => {
    const states = [
      state("backlog", "queue"),
      state("needs_human", "human"),
      state("done", "terminal"),
      state("ready", "queue"),
    ];
    expect(reopenStates(states).map((s) => s.name)).toEqual([
      "backlog",
      "needs_human",
      "ready",
    ]);
  });
});
