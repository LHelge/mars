// `SPEC.md`, "Git (`/api/projects/{pid}/git`)". Merge and rebase answer 422
// with `conflicts` when git could not do the work; `isGitConflict` narrows an
// `ApiError` to that case.

import type {
  CommitResult,
  Diff,
  DiffTarget,
  HistoryEntry,
  HistoryQuery,
  MergeInput,
  PushInput,
  PushResult,
  RebaseInput,
  RevertInput,
  RevertResult,
  SessionBranch,
} from "../types";
import { ApiError, apiGet, apiPost, seg } from "./apiClient";

export function listSessionBranches(pid: string): Promise<SessionBranch[]> {
  return apiGet<SessionBranch[]>(`/projects/${seg(pid)}/git/session-branches`);
}

/**
 * Exactly one of `head` or `handoff_id` (400 otherwise); `base` defaults to
 * the project's default branch. A session `head` is synced first.
 *
 * `signal` is TanStack Query's: a diff is up to a megabyte and syncs the
 * session head server-side before answering, so a panel whose head moved — or
 * that the user closed — abandons the read instead of paying for it twice.
 */
export function getDiff(
  pid: string,
  target: DiffTarget,
  base?: string,
  signal?: AbortSignal,
): Promise<Diff> {
  return apiGet<Diff>(`/projects/${seg(pid)}/git/diff`, {
    query: { ...target, base },
    signal,
  });
}

/**
 * The first-parent history of an integration head, newest first; `before` is
 * the last `commit` of the previous page.
 */
export function listHistory(
  pid: string,
  query: HistoryQuery,
  signal?: AbortSignal,
): Promise<HistoryEntry[]> {
  return apiGet<HistoryEntry[]>(`/projects/${seg(pid)}/git/history`, {
    query: { ...query },
    signal,
  });
}

/**
 * One new commit on `branch` whose tree is `to`'s; 409 `branch has moved`
 * when the head is no longer `expected_head`, and 409 `nothing to revert`
 * when it already has `to`'s tree.
 */
export function revert(pid: string, input: RevertInput): Promise<RevertResult> {
  return apiPost<RevertResult>(`/projects/${seg(pid)}/git/revert`, input);
}

/** The 409 of a revert whose head moved after the confirmation was read. */
export function isBranchMoved(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    error.status === 409 &&
    error.error === "branch has moved"
  );
}

/**
 * The 409 of a revert whose head already has `to`'s content:
 * `nothing to revert: <branch> already matches <short>`.
 */
export function isNothingToRevert(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    error.status === 409 &&
    error.error.startsWith("nothing to revert")
  );
}

export function merge(pid: string, input: MergeInput): Promise<CommitResult> {
  return apiPost<CommitResult>(`/projects/${seg(pid)}/git/merge`, input);
}

export function rebase(pid: string, input: RebaseInput): Promise<CommitResult> {
  return apiPost<CommitResult>(`/projects/${seg(pid)}/git/rebase`, input);
}

export function push(pid: string, input: PushInput): Promise<PushResult> {
  return apiPost<PushResult>(`/projects/${seg(pid)}/git/push`, input);
}

/**
 * True for the 422 a merge or rebase answers with the conflicting paths, so a
 * caller can render them without re-checking `status` and `conflicts` itself.
 */
export function isGitConflict(
  error: unknown,
): error is ApiError & { conflicts: string[] } {
  return (
    error instanceof ApiError &&
    error.status === 422 &&
    Array.isArray(error.conflicts)
  );
}
