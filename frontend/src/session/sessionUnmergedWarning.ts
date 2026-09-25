// What a session's commits not on the default branch mean for ending or
// deleting it, said before the user confirms (`SPEC.md`, "Frontend",
// Confirmations; ADR 0049).
//
// Deleting a session deletes its branch ref `refs/sessions/<sid>` with it, so
// commits the project's default branch does not have are lost. Ending one
// keeps the ref, but the tasks the session filed are not dispatched until its
// commits are on the default branch (`ARCHITECTURE.md`, "Dispatcher"), so they
// wait on a merge the user may not know is due. Both are exactly
// `SessionBranch.ahead`; the confirmation reads it from the project's
// session-branch list, the same cached read the Branches tab shows, rather
// than from anything the delete or end endpoint answers. A warning, never a
// refusal: the user may well have pushed or handed the work off.

import type { SessionBranch } from "../types";

/** Which confirmation the warning is for. */
export type UnmergedPurpose = "delete" | "end";

/** What the commits not on the default branch mean for each purpose. */
const CONSEQUENCE: Record<UnmergedPurpose, string> = {
  delete: "; they will be lost.",
  end: ". Tasks it filed will not be dispatched until they are merged.",
};

/**
 * The sentence naming the commits not on the default branch and what they
 * mean for `purpose`, or `null` when the session's branch has none ahead —
 * including when it has no branch in the list at all, because it never
 * synced, or the list has not been read.
 */
export function sessionUnmergedWarning(
  branches: readonly SessionBranch[] | undefined,
  sessionId: string,
  branchName: string | null,
  purpose: UnmergedPurpose,
): string | null {
  const branch = branches?.find((entry) => entry.session_id === sessionId);
  if (branch === undefined || branch.ahead <= 0) {
    return null;
  }

  const commits = branch.ahead === 1 ? "1 commit" : `${branch.ahead} commits`;
  return `${branchName ?? branch.ref} has ${commits} not on ${branch.base}${CONSEQUENCE[purpose]}`;
}

/** The delete confirmation's sentence about what a delete takes with it. */
export const DELETE_CONSEQUENCES =
  "Its transcript, its events and its branch in the git mirror go with it.";

/** The end confirmation's sentence about what an end does. */
export const END_CONSEQUENCES =
  "The container stops and the agent cannot be given anything more to do.";
