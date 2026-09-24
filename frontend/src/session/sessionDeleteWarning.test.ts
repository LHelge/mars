// The unmerged-commits warning of a session delete (ADR 0049).

import { describe, expect, it } from "vitest";

import type { SessionBranch } from "../types";
import { unmergedCommitsWarning } from "./sessionDeleteWarning";

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

describe("unmergedCommitsWarning", () => {
  it("names the branch, the count and the base", () => {
    expect(unmergedCommitsWarning([branch(3)], SESSION_ID, "session/abc")).toBe(
      "session/abc has 3 commits not on main; they will be lost.",
    );
  });

  it("says one commit in the singular", () => {
    expect(unmergedCommitsWarning([branch(1)], SESSION_ID, "session/abc")).toBe(
      "session/abc has 1 commit not on main; they will be lost.",
    );
  });

  it("falls back to the ref when the session records no branch name", () => {
    expect(unmergedCommitsWarning([branch(2)], SESSION_ID, null)).toBe(
      `refs/sessions/${SESSION_ID} has 2 commits not on main; they will be lost.`,
    );
  });

  it("says nothing for a branch with nothing ahead", () => {
    expect(
      unmergedCommitsWarning([branch(0)], SESSION_ID, "session/abc"),
    ).toBeNull();
  });

  it("says nothing for a session that never synced, or a list not yet read", () => {
    const other = branch(4, "00000000-0000-4000-8000-0000000000d6");
    expect(
      unmergedCommitsWarning([other], SESSION_ID, "session/abc"),
    ).toBeNull();
    expect(
      unmergedCommitsWarning(undefined, SESSION_ID, "session/abc"),
    ).toBeNull();
  });
});
