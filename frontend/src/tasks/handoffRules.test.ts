import { describe, expect, it } from "vitest";

import { ApiError } from "../services/apiClient";
import type { Handoff, Session } from "../types";
import {
  commentExcerpt,
  commitIdError,
  defaultSourceSession,
  handoffComment,
  orderSessionsForPicker,
  reviewCoverLine,
  reviewErrorMessage,
  reviewLabel,
} from "./handoffRules";

const COMMIT = "0123456789abcdef0123456789abcdef01234567";

function handoff(over: Partial<Handoff> = {}): Handoff {
  return {
    id: "h1",
    task_id: "t1",
    source_session_id: "s1",
    source_branch: "refs/sessions/s1",
    commit: COMMIT,
    comment_id: "c1",
    review_status: "unreviewed",
    reviewed_by_user_id: null,
    reviewed_by_session_id: null,
    reviewed_at: null,
    created_by_user_id: "u1",
    created_by_session_id: null,
    created_at: "2026-01-01T00:00:00Z",
    ...over,
  };
}

function session(
  id: string,
  branch: string | null = "refs/sessions/x",
): Session {
  return {
    id,
    project_id: "p1",
    profile_id: "pr1",
    kind: "conversational",
    created_by: null,
    title: null,
    task_id: null,
    handoff_id: null,
    state: "running",
    base_ref: "main",
    branch,
    container_id: null,
    cli_session_id: null,
    last_seq: 0,
    last_activity_at: "2026-01-01T00:00:00Z",
    cost_usd: 0,
    input_tokens: 0,
    output_tokens: 0,
    error: null,
    created_at: "2026-01-01T00:00:00Z",
    parked_at: null,
    ended_at: null,
  };
}

describe("reviewLabel", () => {
  it("says unreviewed without a colour", () => {
    const label = reviewLabel(handoff());
    expect(label.text).toBe("Unreviewed");
    expect(label.tone).toBe("text-console-muted");
  });

  it("names the commit an approval covers", () => {
    expect(reviewLabel(handoff({ review_status: "approved" })).text).toBe(
      "Approved · 0123456789",
    );
  });

  it("names the commit changes were requested on", () => {
    const label = reviewLabel(handoff({ review_status: "changes_requested" }));
    expect(label.text).toBe("Changes requested · 0123456789");
    expect(label.tone).toBe("text-state-parked");
  });
});

describe("commitIdError", () => {
  it("accepts a full lowercase object id", () => {
    expect(commitIdError(COMMIT)).toBeNull();
    expect(commitIdError(`  ${COMMIT}  `)).toBeNull();
  });

  it("asks for a commit when the field is empty", () => {
    expect(commitIdError("   ")).toBe("A commit id is required");
  });

  it("refuses an abbreviation, a branch name and upper case", () => {
    for (const value of [COMMIT.slice(0, 10), "main", COMMIT.toUpperCase()]) {
      expect(commitIdError(value)).toBe(
        "commit must be a full lowercase hexadecimal git object id",
      );
    }
  });
});

describe("orderSessionsForPicker", () => {
  it("offers the sessions that touched the task first, in the task's order", () => {
    const ordered = orderSessionsForPicker(
      [session("a"), session("b"), session("c"), session("d")],
      ["c", "a"],
    );
    expect(ordered.map((row) => row.id)).toEqual(["c", "a", "b", "d"]);
  });

  it("keeps the API's order when nothing touched the task", () => {
    const ordered = orderSessionsForPicker([session("a"), session("b")], []);
    expect(ordered.map((row) => row.id)).toEqual(["a", "b"]);
  });

  it("ignores a touched session the project list no longer has", () => {
    const ordered = orderSessionsForPicker([session("a")], ["gone", "a"]);
    expect(ordered.map((row) => row.id)).toEqual(["a"]);
  });
});

describe("defaultSourceSession", () => {
  it("chooses the only session that has a branch", () => {
    expect(defaultSourceSession([session("a", null), session("b")])).toBe("b");
  });

  it("chooses nothing when more than one candidate exists", () => {
    expect(defaultSourceSession([session("a"), session("b")])).toBe("");
  });

  it("chooses nothing when no session has a branch", () => {
    expect(defaultSourceSession([session("a", null)])).toBe("");
  });
});

describe("reviewErrorMessage", () => {
  it("explains a hand-off that is no longer current", () => {
    const caught = new ApiError(
      409,
      "handoff_id is not the task's current hand-off",
    );
    expect(reviewErrorMessage(caught)).toBe(
      "The hand-off changed; review the new revision",
    );
  });

  it("shows every other refusal in the server's words", () => {
    expect(reviewErrorMessage(new ApiError(409, "project is not ready"))).toBe(
      "project is not ready",
    );
    expect(
      reviewErrorMessage(new ApiError(400, "comment body must not be empty")),
    ).toBe("comment body must not be empty");
  });
});

describe("reviewCoverLine", () => {
  it("names the commit each decision covers", () => {
    expect(reviewCoverLine("approved", COMMIT)).toBe(
      "Approving commit 0123456789",
    );
    expect(reviewCoverLine("changes_requested", COMMIT)).toBe(
      "Requesting changes on commit 0123456789",
    );
    expect(reviewCoverLine("none", COMMIT)).toBe(
      "Forwarding commit 0123456789 with no review decision",
    );
  });
});

describe("handoffComment", () => {
  const comment = {
    id: "c1",
    task_id: "t1",
    author_user_id: "u1",
    author_session_id: null,
    system: false,
    body: "Fixed the parser\n\nDetails follow.",
    created_at: "2026-01-01T00:00:00Z",
  };

  it("resolves the comment the hand-off wrote", () => {
    expect(handoffComment([comment], handoff())?.body).toBe(comment.body);
  });

  it("answers null while the comment is missing from a partial refresh", () => {
    expect(handoffComment([], handoff())).toBeNull();
    expect(handoffComment([comment], handoff({ comment_id: null }))).toBeNull();
  });
});

describe("commentExcerpt", () => {
  it("takes the first line", () => {
    expect(commentExcerpt("Fixed the parser\n\nDetails.")).toBe(
      "Fixed the parser",
    );
  });

  it("truncates a long line", () => {
    expect(commentExcerpt("x".repeat(200))).toBe(`${"x".repeat(90)}…`);
  });
});
