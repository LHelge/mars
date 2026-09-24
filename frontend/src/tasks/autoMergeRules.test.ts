import { describe, expect, it } from "vitest";

import { ApiError } from "../services/apiClient";
import type { TaskState, TaskStateKind } from "../types";
import {
  autoMergeChanged,
  autoMergeRefusal,
  conflictCandidates,
  defaultConflictState,
  toAutoMergeDraft,
  toAutoMergeUpdate,
  toggleAutoMerge,
} from "./autoMergeRules";

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

const BACKLOG = state("backlog", "queue", 0);
const READY = state("ready", "queue", 1);
const MERGE = state("merge", "queue", 2);
const NEEDS_HUMAN = state("needs_human", "human", 3);
const DONE = state("done", "terminal", 4);
const DEFAULTS = [BACKLOG, READY, MERGE, NEEDS_HUMAN, DONE];

describe("conflictCandidates", () => {
  it("offers the other queue states in position order, never itself, human or terminal", () => {
    const shuffled = [DONE, MERGE, NEEDS_HUMAN, READY, BACKLOG];

    expect(conflictCandidates(MERGE, shuffled).map((s) => s.name)).toEqual([
      "backlog",
      "ready",
    ]);
  });
});

describe("defaultConflictState", () => {
  it("pre-selects ready when the project has it", () => {
    expect(defaultConflictState(MERGE, DEFAULTS)).toBe("ready");
  });

  it("falls back to the first other queue state without a ready one", () => {
    expect(defaultConflictState(MERGE, [BACKLOG, MERGE, DONE])).toBe("backlog");
    // `ready` itself cannot be its own conflict state.
    expect(defaultConflictState(READY, DEFAULTS)).toBe("backlog");
  });

  it("has nothing to offer a lone queue state", () => {
    expect(defaultConflictState(MERGE, [MERGE, NEEDS_HUMAN, DONE])).toBeNull();
  });
});

describe("toggleAutoMerge", () => {
  it("turning it on pre-selects the default", () => {
    const draft = toggleAutoMerge(
      toAutoMergeDraft(MERGE),
      true,
      MERGE,
      DEFAULTS,
    );

    expect(draft).toEqual({ autoMerge: true, conflictState: "ready" });
  });

  it("keeps a choice made before the toggle went off and back on", () => {
    const chosen = { autoMerge: true, conflictState: "backlog" };
    const off = toggleAutoMerge(chosen, false, MERGE, DEFAULTS);
    const on = toggleAutoMerge(off, true, MERGE, DEFAULTS);

    expect(off.autoMerge).toBe(false);
    expect(on).toEqual(chosen);
  });
});

describe("toAutoMergeUpdate", () => {
  it("sends the pair together when on", () => {
    expect(
      toAutoMergeUpdate({ autoMerge: true, conflictState: "ready" }),
    ).toEqual({ auto_merge: true, conflict_state: "ready" });
  });

  it("sends an explicit null conflict state when off, whatever the draft held", () => {
    expect(
      toAutoMergeUpdate({ autoMerge: false, conflictState: "ready" }),
    ).toEqual({ auto_merge: false, conflict_state: null });
  });
});

describe("autoMergeChanged", () => {
  const stored: TaskState = {
    ...MERGE,
    auto_merge: true,
    conflict_state: "ready",
  };

  it("is false for the stored pair and true for a different one", () => {
    expect(autoMergeChanged(toAutoMergeDraft(stored), stored)).toBe(false);
    expect(
      autoMergeChanged({ autoMerge: true, conflictState: "backlog" }, stored),
    ).toBe(true);
    expect(
      autoMergeChanged({ autoMerge: false, conflictState: "ready" }, stored),
    ).toBe(true);
  });

  it("ignores a leftover name while off", () => {
    expect(
      autoMergeChanged({ autoMerge: false, conflictState: "ready" }, MERGE),
    ).toBe(false);
  });
});

describe("autoMergeRefusal", () => {
  it("words the exact 400s of SPEC.md", () => {
    expect(
      autoMergeRefusal(
        new ApiError(400, "conflict_state must be a different state"),
      ),
    ).toBe("The conflict state must be a different state.");
    expect(
      autoMergeRefusal(new ApiError(400, "auto_merge requires conflict_state")),
    ).toBe("Choose the state a conflicting merge sends the task to.");
  });

  it("leaves anything else to errorMessage", () => {
    expect(autoMergeRefusal(new ApiError(409, "name taken"))).toBe(
      "name taken",
    );
    // A server text that happens to be an Object prototype member is not a
    // mapped refusal.
    expect(autoMergeRefusal(new ApiError(400, "constructor"))).toBe(
      "constructor",
    );
  });
});
