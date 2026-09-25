// The unmerged-commits warning of a session end or delete (ADR 0049).

import { describe, expect, it } from "vitest";

import type { SessionBranch } from "../types";
import { sessionUnmergedWarning } from "./sessionUnmergedWarning";

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const SESSION_ID = "00000000-0000-4000-8000-0000000000d5";

function branch(ahead: number, sessionId = SESSION_ID): SessionBranch {
  return {
    session_id: sessionId,
    ref: `refs/sessions/${sessionId}`,
    commit: "b".repeat(40),
    ahead,
    behind: 2,
    base: "main",
    updated_at: "2026-03-01T11:00:00Z",
  };
}

describe("sessionUnmergedWarning", () => {
  it("names the branch, the count and the base for a delete", () => {
    expect(
      sessionUnmergedWarning([branch(3)], SESSION_ID, "session/abc", "delete"),
    ).toBe("session/abc has 3 commits not on main; they will be lost.");
  });

  it("says one commit in the singular for a delete", () => {
    expect(
      sessionUnmergedWarning([branch(1)], SESSION_ID, "session/abc", "delete"),
    ).toBe("session/abc has 1 commit not on main; they will be lost.");
  });

  it("says what an end holds back", () => {
    expect(
      sessionUnmergedWarning([branch(3)], SESSION_ID, "session/abc", "end"),
    ).toBe(
      "session/abc has 3 commits not on main. Tasks it filed will not be dispatched until they are merged.",
    );
  });

  it("says one commit in the singular for an end", () => {
    expect(
      sessionUnmergedWarning([branch(1)], SESSION_ID, "session/abc", "end"),
    ).toBe(
      "session/abc has 1 commit not on main. Tasks it filed will not be dispatched until they are merged.",
    );
  });

  it("falls back to the ref when the session records no branch name", () => {
    expect(
      sessionUnmergedWarning([branch(2)], SESSION_ID, null, "delete"),
    ).toBe(
      `refs/sessions/${SESSION_ID} has 2 commits not on main; they will be lost.`,
    );
  });

  it("says nothing for a branch with nothing ahead", () => {
    for (const purpose of ["delete", "end"] as const) {
      expect(
        sessionUnmergedWarning([branch(0)], SESSION_ID, "session/abc", purpose),
      ).toBeNull();
    }
  });

  it("says nothing for a session that never synced, or a list not yet read", () => {
    const other = branch(4, "00000000-0000-4000-8000-0000000000d6");
    for (const purpose of ["delete", "end"] as const) {
      expect(
        sessionUnmergedWarning([other], SESSION_ID, "session/abc", purpose),
      ).toBeNull();
      expect(
        sessionUnmergedWarning(undefined, SESSION_ID, "session/abc", purpose),
      ).toBeNull();
    }
  });
});
