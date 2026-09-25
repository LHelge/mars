// Why a task nobody has picked up may be waiting: its author session's work is
// not on the default branch yet (`SPEC.md`, "Frontend", Task board; the rule is
// the dispatcher's, `ARCHITECTURE.md`, "Dispatcher" → "A task waits for its
// author's work", ADR 0052).
//
// Pure, so the card, the drawer and their tests read the one rule. It joins a
// task's `created_by_session_id` with the project's session-branch list
// (`GET /projects/{pid}/git/session-branches`), whose `ahead` is measured
// against the default branch: `ahead > 0` is the dispatcher's "tip not
// contained in the default branch". A session with no ref in that list —
// deleted, ended with nothing unique, or live but never synced — says nothing
// here; the dispatcher's own check syncs such a session silently, so the board
// catches up once a dispatcher run has created the ref.

import type { SessionBranch, Task } from "../types";

/** The author branch a task is waiting for. */
export interface AuthorBranchWait {
  sessionId: string;
  /** Commits on the session's branch that are not on `base`. */
  ahead: number;
  /** The default branch the counts are measured against. */
  base: string;
}

/**
 * Whether the dispatcher's author rule could be what keeps `task` waiting.
 *
 * Only a task the dispatcher could still launch is asked about: one that is
 * not closed (a terminal state sets `closed_at`) and that no session holds.
 */
export function waitsForAuthorBranch(
  task: Pick<
    Task,
    "created_by_session_id" | "closed_at" | "lease_holder_session_id"
  >,
): boolean {
  return (
    task.created_by_session_id !== null &&
    task.closed_at === null &&
    task.lease_holder_session_id === null
  );
}

/**
 * The author branch `task` is waiting for, or `null` when it waits for none —
 * including while the branch list is not loaded, because the line informs and
 * never guesses.
 */
export function authorBranchWait(
  task: Pick<
    Task,
    "created_by_session_id" | "closed_at" | "lease_holder_session_id"
  >,
  branches: readonly SessionBranch[] | undefined,
): AuthorBranchWait | null {
  if (!waitsForAuthorBranch(task) || branches === undefined) {
    return null;
  }
  const row = branches.find(
    (branch) => branch.session_id === task.created_by_session_id,
  );
  if (row === undefined || row.ahead <= 0) {
    return null;
  }
  return { sessionId: row.session_id, ahead: row.ahead, base: row.base };
}

/**
 * The line itself: `Waiting for session <title>'s branch to reach <base>
 * (N commits)`. An untitled session is named by the first eight characters of
 * its id, as the Branches tab names it.
 */
export function authorBranchMessage(
  wait: AuthorBranchWait,
  title: string | null | undefined,
): string {
  const name =
    title === null || title === undefined || title === ""
      ? wait.sessionId.slice(0, 8)
      : title;
  const commits =
    wait.ahead === 1 ? "1 commit" : `${String(wait.ahead)} commits`;
  return `Waiting for session ${name}'s branch to reach ${wait.base} (${commits})`;
}
