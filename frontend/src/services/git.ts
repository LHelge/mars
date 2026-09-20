// `SPEC.md`, "Git (`/api/projects/{pid}/git`)". Merge and rebase answer 422
// with `conflicts` when git could not do the work; `isGitConflict` narrows an
// `ApiError` to that case.

import type {
  CommitResult,
  Diff,
  DiffTarget,
  MergeInput,
  PushInput,
  PushResult,
  RebaseInput,
  SessionBranch,
} from "../types";
import { ApiError, apiGet, apiPost } from "./apiClient";

export function listSessionBranches(pid: string): Promise<SessionBranch[]> {
  return apiGet<SessionBranch[]>(`/projects/${pid}/git/session-branches`);
}

/**
 * Exactly one of `head` or `handoff_id` (400 otherwise); `base` defaults to
 * the project's default branch. A session `head` is synced first.
 */
export function getDiff(
  pid: string,
  target: DiffTarget,
  base?: string,
): Promise<Diff> {
  const query = new URLSearchParams(
    "head" in target ? { head: target.head } : { handoff_id: target.handoff_id },
  );
  if (base !== undefined) {
    query.set("base", base);
  }
  return apiGet<Diff>(`/projects/${pid}/git/diff?${query.toString()}`);
}

export function merge(pid: string, input: MergeInput): Promise<CommitResult> {
  return apiPost<CommitResult>(`/projects/${pid}/git/merge`, input);
}

export function rebase(pid: string, input: RebaseInput): Promise<CommitResult> {
  return apiPost<CommitResult>(`/projects/${pid}/git/rebase`, input);
}

export function push(pid: string, input: PushInput): Promise<PushResult> {
  return apiPost<PushResult>(`/projects/${pid}/git/push`, input);
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
