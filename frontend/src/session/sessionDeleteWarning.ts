// What deleting a session costs in git, said before the user confirms it
// (`SPEC.md`, "Frontend", Confirmations; ADR 0049).
//
// Deleting a session deletes its branch ref `refs/sessions/<sid>` with it. That
// is only a loss when the branch holds commits the project's default branch
// does not, which is exactly `SessionBranch.ahead`; the confirmation reads it
// from the project's session-branch list, the same cached read the Branches
// tab shows, rather than from anything the delete endpoint answers. A warning,
// never a refusal: the user may well have pushed or handed the work off.

import type { SessionBranch } from "../types";

/**
 * The sentence naming the commits a delete would lose, or `null` when the
 * session's branch has none ahead of the default branch — including when it
 * has no branch in the list at all, because it never synced.
 */
export function unmergedCommitsWarning(
  branches: readonly SessionBranch[] | undefined,
  sessionId: string,
  branchName: string | null,
): string | null {
  const branch = branches?.find((entry) => entry.session_id === sessionId);
  if (branch === undefined || branch.ahead <= 0) {
    return null;
  }

  const commits = branch.ahead === 1 ? "1 commit" : `${branch.ahead} commits`;
  return `${branchName ?? branch.ref} has ${commits} not on ${branch.base}; they will be lost.`;
}

/** The confirmation's sentence about what a delete takes with it. */
export const DELETE_CONSEQUENCES =
  "Its transcript, its events and its branch in the git mirror go with it.";
