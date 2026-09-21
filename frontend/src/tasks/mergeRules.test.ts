// The merge button mirrors the server's approval gate, so the predicate is
// tested against every review status a hand-off can be in, and the conflict
// parsing against the two failures the endpoint distinguishes: 422 with paths
// and 409 without.

import { describe, expect, it } from "vitest";

import { ApiError } from "../services/apiClient";
import type { Handoff, ReviewStatus, TaskDetail } from "../types";
import {
  canMerge,
  isStaleMerge,
  mergeConflict,
  mergeCoverLine,
  mergeErrorMessage,
  mergedMessage,
} from "./mergeRules";

// Obviously fake fixture values (CLAUDE.md, rule 3).
const COMMIT = "abcdef0123456789abcdef0123456789abcdef01";

function handoff(review_status: ReviewStatus): Handoff {
  return {
    id: "00000000-0000-4000-8000-0000000000c3",
    commit: COMMIT,
    source_branch: "sessions/fix-login",
    review_status,
  } as Handoff;
}

function task(current: Handoff | null): Pick<TaskDetail, "handoff"> {
  return { handoff: current };
}

describe("canMerge", () => {
  it("refuses a task with no hand-off", () => {
    expect(canMerge(task(null))).toBe(false);
  });

  it("refuses an unreviewed hand-off, which every new revision is", () => {
    expect(canMerge(task(handoff("unreviewed")))).toBe(false);
  });

  it("refuses a hand-off with changes requested", () => {
    expect(canMerge(task(handoff("changes_requested")))).toBe(false);
  });

  it("allows an approved current hand-off", () => {
    expect(canMerge(task(handoff("approved")))).toBe(true);
  });
});

describe("mergeConflict", () => {
  it("reads the paths and the sentence off a 422", () => {
    const error = new ApiError(422, "merge conflict", [
      "src/one.ts",
      "src/two.ts",
    ]);

    expect(mergeConflict(error)).toEqual({
      paths: ["src/one.ts", "src/two.ts"],
      message: "merge conflict",
    });
  });

  it("is not a conflict without the paths, whatever the status", () => {
    expect(mergeConflict(new ApiError(422, "merge conflict"))).toBeNull();
    expect(
      mergeConflict(new ApiError(409, "hand-off is not approved")),
    ).toBeNull();
    expect(mergeConflict(new Error("offline"))).toBeNull();
  });
});

describe("isStaleMerge", () => {
  it("is the 409 of a superseded or unapproved hand-off", () => {
    expect(
      isStaleMerge(
        new ApiError(409, "handoff_id is not the task's current hand-off"),
      ),
    ).toBe(true);
    expect(isStaleMerge(new ApiError(409, "hand-off is not approved"))).toBe(
      true,
    );
  });

  it("is not any other failure", () => {
    expect(isStaleMerge(new ApiError(422, "merge conflict", []))).toBe(false);
    expect(isStaleMerge(new Error("offline"))).toBe(false);
  });
});

describe("mergeErrorMessage", () => {
  it("uses the server's words", () => {
    expect(
      mergeErrorMessage(new ApiError(409, "hand-off is not approved")),
    ).toBe("hand-off is not approved");
  });

  it("falls back for anything that is not an API answer", () => {
    expect(mergeErrorMessage(new Error("offline"))).toBe(
      "Something went wrong",
    );
  });
});

describe("wording", () => {
  it("names the commit and the branch whose later commits are left out", () => {
    expect(mergeCoverLine(handoff("approved"))).toBe(
      "Merges commit abcdef0 exactly; later commits on sessions/fix-login are not included",
    );
  });

  it("says the merge did not move the task", () => {
    expect(mergedMessage(COMMIT)).toBe(
      "Merged as abcdef0 · Task state unchanged",
    );
  });
});
