// Mirrors `SPEC.md`, "Git (`/api/projects/{pid}/git`)". Ahead/behind is
// measured against the Mars integration head named by the project's
// `default_branch`.

import type { Task } from "./tasks";

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

/**
 * The letters `git diff --name-status` prints, as the orchestrator serialises
 * them (`orchestrator/src/models/git.rs`, `DiffStatus`).
 *
 * `R` and `C` are part of the documented shape and not produced in v1: the
 * diff is taken with `--no-renames` so that the status list and the line counts
 * describe the same set of paths. They stay in the type because they are what
 * the endpoint may answer, and a lookup table that covers them costs one line
 * each; leaving them out would mean a status the server can send has no
 * rendering at all.
 */
export const DIFF_STATUSES = ["A", "M", "D", "R", "C", "T"] as const;

export type DiffStatus = (typeof DIFF_STATUSES)[number];

export interface DiffFile {
  path: string;
  status: DiffStatus;
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

/** A task an entry of the history brought in, through one of its hand-offs. */
export interface HistoryTask {
  id: string;
  number: number;
  title: string;
  /** The newest hand-off record of the task pinning the commit brought in. */
  handoff_id: string;
}

/** A session behind an entry of the history. */
export interface HistorySession {
  id: string;
  title: string | null;
}

/**
 * One commit on the first-parent line of an integration head
 * (`GET /projects/{pid}/git/history`), newest first.
 */
export interface HistoryEntry {
  commit: string;
  /** The first parent first; empty for a root commit. */
  parents: string[];
  /** The commit's tree id: the head's when this entry holds its current content. */
  tree: string;
  /** The message's first line. */
  subject: string;
  author_name: string;
  /** The committer date. */
  committed_at: string;
  /**
   * The commit's `Requested-By` trailer — `user:<id>`, `session:<id>` or
   * `system` — or null. Read through `parseRequestedBy`
   * (`components/git/history.ts`), which keeps an unknown spelling as text.
   */
  requested_by: string | null;
  /**
   * The newest revert commit above this entry on the first-parent line that
   * undid it, or null. Read through `historyRowMark`.
   */
  reverted_by: string | null;
  tasks: HistoryTask[];
  sessions: HistorySession[];
}

/** `GET /projects/{pid}/git/history`'s query. */
export interface HistoryQuery {
  /** An integration head; the project's default branch when omitted. */
  branch?: string;
  /** The last `commit` of the previous page, which must be on the first-parent line. */
  before?: string;
  /** At most 200 (a larger value is reduced to it); the server's default is 50. */
  limit?: number;
}

/** `POST /projects/{pid}/git/revert`. */
export interface RevertInput {
  /** An integration head. */
  branch: string;
  /** A full object id on the head's first-parent line, strictly older than the head. */
  to: string;
  /** The head the confirmation was read at; 409 `branch has moved` otherwise. */
  expected_head: string;
  /** Move the terminal tasks of the reverted range to a queue or human state. */
  reopen?: { state: string; comment: string };
}

export interface RevertResult {
  /** The new commit on `branch`, whose tree is `to`'s. */
  commit: string;
  /** The first-parent range `to..head`, newest first. */
  reverted: HistoryEntry[];
  /** The tasks moved, empty without `reopen`. */
  reopened: Task[];
}
