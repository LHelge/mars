// Mirrors `SPEC.md`, "Git (`/api/projects/{pid}/git`)". Ahead/behind is
// measured against the Mars integration head named by the project's
// `default_branch`.

export interface SessionBranch {
  session_id: string;
  /** `refs/sessions/<session id>`. */
  ref: string;
  commit: string;
  ahead: number;
  behind: number;
  /** The integration head the counts are measured against. */
  base: string;
  updated_at: string;
}

export interface DiffFile {
  path: string;
  status: string;
  additions: number;
  deletions: number;
}

/** `patch` is the unified diff from `merge_base` to `head`, truncated above 1 MiB. */
export interface Diff {
  base: string;
  head: string;
  merge_base: string;
  files: DiffFile[];
  patch: string;
  truncated: boolean;
}

/**
 * `GET /projects/{pid}/git/diff` takes exactly one of `head` or `handoff_id`
 * (400 otherwise), so the two are modelled as alternatives a caller cannot
 * combine. `base` defaults to the project's default branch.
 */
export type DiffTarget = { head: string } | { handoff_id: string };

/**
 * The branch form is a generic git action; the task form merges the exact
 * commit retained for the task's current hand-off, which must be `approved`.
 */
export type MergeInput = { target: string; message?: string } & (
  | { source: string; task_id?: never; handoff_id?: never }
  | { source?: never; task_id: string; handoff_id: string }
);

export interface RebaseInput {
  /** A session id or integration branch name. */
  branch: string;
  /** An integration or upstream-tracking branch name. */
  onto: string;
}

export interface PushInput {
  /** A session id or integration branch name; upstream-tracking refs are refused. */
  ref: string;
  remote_branch?: string;
  /** A force push must be asked for explicitly. */
  force?: boolean;
}

export interface PushResult {
  remote_branch: string;
  commit: string;
}

/** Merge and rebase answer the resulting commit, or 422 with `conflicts`. */
export interface CommitResult {
  commit: string;
}

/** The 422 body of a conflicting merge or rebase (`SPEC.md`, "REST API"). */
export interface GitConflictError {
  status: 422;
  error: string;
  conflicts: string[];
}
