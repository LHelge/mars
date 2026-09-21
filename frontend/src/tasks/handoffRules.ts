// The decisions behind the drawer's hand-off panel, kept out of the components
// so they can be read and tested on their own (`SPEC.md`, "Code hand-offs and
// review"; `ARCHITECTURE.md`, "Task tracker", "Code hand-offs" and "Review
// approval").
//
// Three of them matter enough to name:
//
// 1. A review decision is always about a commit, never about a task. The badge
//    therefore carries the commit it covers, and a new revision resets the
//    status to `unreviewed` however many approvals precede it in history.
// 2. The commit is a full object id, not a moving ref. The form refuses
//    anything else before the request, in the API's own words, because the
//    server's `commit must be a full lowercase hexadecimal git object id` is
//    the same sentence a second later.
// 3. The source of a hand-off is a session of the project, and which one is a
//    choice the user makes. Sessions that already touched the task are offered
//    first because they are the likely answer, never as a silent default.

import type { Comment, Handoff, ReviewStatus, Session } from "../types";
import { ApiError } from "../services/apiClient";
import { errorMessage } from "../services/errorMessage";
import { shortCommit } from "./launchRules";

/** The commit id the API accepts: a full, lowercase, hexadecimal object id. */
export const COMMIT_RULE = /^[0-9a-f]{40}$/;

/** What the commit field says about itself, before anything is typed into it. */
export const COMMIT_HINT =
  "Full 40-character commit id; the session branch tip must equal it";

/** What the state select says about itself. */
export const STATE_HINT = "A hand-off requires a different target state";

/**
 * Why this commit id cannot be sent, or `null` when it can. The wording is the
 * server's own 400 (`SPEC.md`, "Code hand-offs and review"), so a value the
 * form lets through and a value it stops read the same.
 */
export function commitIdError(value: string): string | null {
  if (value.trim() === "") {
    return "A commit id is required";
  }
  if (!COMMIT_RULE.test(value.trim())) {
    return "commit must be a full lowercase hexadecimal git object id";
  }
  return null;
}

/** The review badge: what it says, and the one colour the panel spends. */
export interface ReviewLabel {
  text: string;
  /** A Tailwind colour class; `unreviewed` is deliberately not a colour. */
  tone: string;
  status: ReviewStatus;
}

const TONE: Record<ReviewStatus, string> = {
  unreviewed: "text-console-muted",
  approved: "text-state-running",
  changes_requested: "text-state-parked",
};

/**
 * The review status of one hand-off, labelled with the commit the decision
 * covers. An approval never transfers to a later revision, so reading the
 * commit off the badge is how a reviewer knows which code was approved.
 */
export function reviewLabel(
  handoff: Pick<Handoff, "commit" | "review_status">,
): ReviewLabel {
  const short = shortCommit(handoff.commit);
  switch (handoff.review_status) {
    case "approved":
      return {
        text: `Approved · ${short}`,
        tone: TONE.approved,
        status: "approved",
      };
    case "changes_requested":
      return {
        text: `Changes requested · ${short}`,
        tone: TONE.changes_requested,
        status: "changes_requested",
      };
    case "unreviewed":
      return {
        text: "Unreviewed",
        tone: TONE.unreviewed,
        status: "unreviewed",
      };
    default: {
      // A `ReviewStatus` this build does not know must not be labelled
      // "Unreviewed": the badge is the reviewer's statement about which code
      // was approved, and a wrong one is worse than an unfamiliar one.
      const unhandled: never = handoff.review_status;
      return {
        text: String(unhandled),
        tone: TONE.unreviewed,
        status: "unreviewed",
      };
    }
  }
}

/**
 * The project's sessions as the revision form offers them: the ones that have
 * touched this task first, in the order the task lists them, then everything
 * else in the order the API sent it. Nothing is removed — a session without a
 * branch is still shown, and refused with its own reason at the point of
 * choosing — because a list that quietly omits the session the user is looking
 * for is worse than one that explains why it cannot be used.
 */
export function orderSessionsForPicker(
  sessions: readonly Session[],
  touched: readonly string[],
): Session[] {
  const rank = new Map<string, number>();
  touched.forEach((id, index) => {
    if (!rank.has(id)) {
      rank.set(id, index);
    }
  });

  const first: Session[] = [];
  const rest: Session[] = [];
  for (const session of sessions) {
    (rank.has(session.id) ? first : rest).push(session);
  }
  first.sort(
    (left, right) => (rank.get(left.id) ?? 0) - (rank.get(right.id) ?? 0),
  );
  return [...first, ...rest];
}

/**
 * The session the revision form starts on. Exactly one usable candidate is a
 * choice with no alternative, so it is made; anything else is left empty,
 * because defaulting to a session the viewer happens to have open is how a
 * reviewer hands off their own branch without noticing (`SPEC.md`,
 * "Frontend", "Hand-off controls": neither action silently selects the
 * reviewer's own branch).
 */
export function defaultSourceSession(sessions: readonly Session[]): string {
  const usable = sessions.filter((session) => session.branch !== null);
  return usable.length === 1 ? usable[0].id : "";
}

/** The exact 409 that means the hand-off moved while the review form was open. */
const STALE_HANDOFF = "handoff_id is not the task's current hand-off";

/**
 * What a failed review says. Every refusal is shown in the server's own words
 * except the one the user can do nothing about by retrying: a hand-off that is
 * no longer current means a new revision arrived, and the answer is to read it
 * (`SPEC.md`, "Code hand-offs and review": forwarding requires the current
 * `handoff_id`).
 */
export function reviewErrorMessage(caught: unknown): string {
  if (
    caught instanceof ApiError &&
    caught.status === 409 &&
    caught.error === STALE_HANDOFF
  ) {
    return "The hand-off changed; review the new revision";
  }
  return errorMessage(caught);
}

/** Which decision a review forwards, if any. */
export type ReviewDecision = "approved" | "changes_requested" | "none";

/** What each decision is called, on its button and on its form. */
export const REVIEW_ACTION: Record<ReviewDecision, string> = {
  approved: "Approve",
  changes_requested: "Request changes",
  none: "Forward without decision",
};

/**
 * What the review form says it is about to do, naming the commit the decision
 * covers. Forwarding reuses the hand-off's pinned commit, so this is the code
 * being judged whatever the source branch has done since.
 */
export function reviewCoverLine(
  decision: ReviewDecision,
  commit: string,
): string {
  const short = shortCommit(commit);
  switch (decision) {
    case "approved":
      return `Approving commit ${short}`;
    case "changes_requested":
      return `Requesting changes on commit ${short}`;
    case "none":
      return `Forwarding commit ${short} with no review decision`;
    default: {
      // A decision this build does not know is not "no decision": the line
      // says what the reviewer is about to record, so it names the decision
      // rather than claiming one that is not being made.
      const unhandled: never = decision;
      return `Recording ${String(unhandled)} on commit ${short}`;
    }
  }
}

/** How much of a hand-off comment one history row carries. */
const EXCERPT = 90;

/** The comment a hand-off wrote, or `null` while the refresh is partial. */
export function handoffComment(
  comments: readonly Comment[],
  handoff: Pick<Handoff, "comment_id">,
): Comment | null {
  if (handoff.comment_id === null) {
    return null;
  }
  return comments.find((comment) => comment.id === handoff.comment_id) ?? null;
}

/** A comment's first line, short enough to sit on a timeline row. */
export function commentExcerpt(body: string): string {
  const line = body.trim().split("\n")[0]?.trim() ?? "";
  return line.length > EXCERPT ? `${line.slice(0, EXCERPT)}…` : line;
}
