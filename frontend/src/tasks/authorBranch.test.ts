import { describe, expect, it } from "vitest";

import type { SessionBranch } from "../types";
import {
  authorBranchMessage,
  authorBranchWait,
  waitsForAuthorBranch,
} from "./authorBranch";

// Obviously fake ids and commit (`CLAUDE.md`, rule 3).
const AUTHOR = "00000000-0000-4000-8000-00000000000a";
const OTHER = "00000000-0000-4000-8000-00000000000b";
const HOLDER = "00000000-0000-4000-8000-00000000000c";

function branch(sessionId: string, ahead: number): SessionBranch {
  return {
    session_id: sessionId,
    ref: `refs/sessions/${sessionId}`,
    commit: "0123456789abcdef0123456789abcdef01234567",
    ahead,
    behind: 0,
    base: "main",
    updated_at: "2026-09-25T12:00:00Z",
  };
}

const OPEN = {
  created_by_session_id: AUTHOR,
  closed_at: null,
  lease_holder_session_id: null,
};

describe("authorBranchWait", () => {
  it("names the author branch while it is ahead of the default branch", () => {
    expect(
      authorBranchWait(OPEN, [branch(OTHER, 5), branch(AUTHOR, 3)]),
    ).toEqual({ sessionId: AUTHOR, ahead: 3, base: "main" });
  });

  it("is nothing once the branch is contained in the default branch", () => {
    expect(authorBranchWait(OPEN, [branch(AUTHOR, 0)])).toBeNull();
  });

  it("is nothing while the author has no ref, and says nothing before the list loads", () => {
    expect(authorBranchWait(OPEN, [branch(OTHER, 2)])).toBeNull();
    expect(authorBranchWait(OPEN, [])).toBeNull();
    expect(authorBranchWait(OPEN, undefined)).toBeNull();
  });

  it("is nothing for a task nobody filed from a session", () => {
    const task = { ...OPEN, created_by_session_id: null };
    expect(waitsForAuthorBranch(task)).toBe(false);
    expect(authorBranchWait(task, [branch(AUTHOR, 3)])).toBeNull();
  });

  it("is nothing for a closed task", () => {
    const task = { ...OPEN, closed_at: "2026-09-25T13:00:00Z" };
    expect(waitsForAuthorBranch(task)).toBe(false);
    expect(authorBranchWait(task, [branch(AUTHOR, 3)])).toBeNull();
  });

  it("is nothing for a task a session holds", () => {
    const task = { ...OPEN, lease_holder_session_id: HOLDER };
    expect(waitsForAuthorBranch(task)).toBe(false);
    expect(authorBranchWait(task, [branch(AUTHOR, 3)])).toBeNull();
  });
});

describe("authorBranchMessage", () => {
  const wait = { sessionId: AUTHOR, ahead: 3, base: "main" };

  it("names the session by its title and counts the commits", () => {
    expect(authorBranchMessage(wait, "Plan the epic")).toBe(
      "Waiting for session Plan the epic's branch to reach main (3 commits)",
    );
  });

  it("falls back to the short id and says one commit in the singular", () => {
    expect(authorBranchMessage({ ...wait, ahead: 1 }, null)).toBe(
      `Waiting for session ${AUTHOR.slice(0, 8)}'s branch to reach main (1 commit)`,
    );
    expect(authorBranchMessage(wait, undefined)).toContain(AUTHOR.slice(0, 8));
  });
});
