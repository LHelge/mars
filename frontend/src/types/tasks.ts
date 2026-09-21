// Mirrors `SPEC.md`, "Tasks (`/api/projects/{pid}/tasks`)", "Code hand-offs and
// review" and "TaskEvent"; `TaskDependencyKind` is the `task_dependency_kind`
// enum of `docs/data-model.md`.

import type { TaskState } from "./taskStates";

export type TaskDependencyKind = "blocks" | "discovered_from" | "related";

export type ReviewStatus = "unreviewed" | "approved" | "changes_requested";

/** 0 (critical) to 3 (low), default 2; a number, never a name. */
export type TaskPriority = 0 | 1 | 2 | 3;

export interface Handoff {
  id: string;
  task_id: string;
  source_session_id: string | null;
  source_branch: string;
  commit: string;
  comment_id: string | null;
  review_status: ReviewStatus;
  reviewed_by_user_id: string | null;
  reviewed_by_session_id: string | null;
  reviewed_at: string | null;
  created_by_user_id: string | null;
  created_by_session_id: string | null;
  created_at: string;
}

export interface TaskComment {
  id: string;
  task_id: string;
  author_user_id: string | null;
  author_session_id: string | null;
  system: boolean;
  body: string;
  created_at: string;
}

export interface TaskDependency {
  task_id: string;
  kind: TaskDependencyKind;
}

export interface Task {
  id: string;
  project_id: string;
  number: number;
  title: string;
  description: string | null;
  /** The state's name, defined per project by the task-state rows. */
  state: string;
  /** 0 (critical) to 3 (low), default 2. */
  priority: TaskPriority;
  blocked: boolean;
  labels: string[];
  parent_id: string | null;
  assignee_user_id: string | null;
  lease_holder_session_id: string | null;
  lease_since: string | null;
  attempts: number;
  needs_human_reason: string | null;
  handoff: Handoff | null;
  depends_on: TaskDependency[];
  /** Ids of the tasks that have a `blocks` dependency on this one. */
  blocks: string[];
  created_at: string;
  updated_at: string;
  closed_at: string | null;
}

/** A row of `TaskDetail.sessions`: a session that touched the task. */
export interface TaskSessionTouch {
  session_id: string;
  first_touched_at: string;
  last_touched_at: string;
}

/** `GET /projects/{pid}/tasks/{id}`; hand-offs are ordered oldest first. */
export interface TaskDetail extends Task {
  comments: TaskComment[];
  handoffs: Handoff[];
  children: Task[];
  sessions: TaskSessionTouch[];
}

// `SPEC.md`, "Code hand-offs and review": the hand-off carried by a `PUT`,
// which always moves the task to a different state and always carries a
// comment. A revision publishes a fresh commit; a forward passes the current
// hand-off on, optionally recording a review decision on it.

/** Publication of a new commit from a session's synced branch. */
export interface RevisionHandoffInput {
  kind: "revision";
  /** Required over REST: a session of this task's project with a synced branch. */
  source_session_id?: string;
  /** A full lowercase hexadecimal git object id, not a moving ref. */
  commit: string;
  comment: string;
}

/** Passing the task's current hand-off on, with an optional review decision. */
export interface ForwardHandoffInput {
  kind: "forward";
  /** Must equal the task's current `handoff.id` (409 otherwise). */
  handoff_id: string;
  comment: string;
  review?: "approved" | "changes_requested";
}

export type HandoffInput = RevisionHandoffInput | ForwardHandoffInput;

// Request bodies of `SPEC.md`, "Tasks" and "Task states". An omitted field is
// left alone; an explicit `null` where the shape allows one clears the column.

export interface CreateTaskInput {
  title: string;
  description?: string;
  /** A state name of the project; omitted means its first `queue` state. */
  state?: string;
  priority?: TaskPriority;
  labels?: string[];
  parent_id?: string;
  /** Task ids or numbers; each becomes a `blocks` dependency. */
  depends_on?: string[];
}

export interface UpdateTaskInput {
  title?: string;
  description?: string;
  state?: string;
  priority?: TaskPriority;
  labels?: string[];
  /** `null` makes the task top-level again. */
  parent_id?: string | null;
  /** `null` unassigns the task. */
  assignee_user_id?: string | null;
  /** Requires a different target `state` in the same update. */
  handoff?: HandoffInput;
}

// `SPEC.md`, "TaskEvent": the SSE payload of the project's task stream. The
// board deduplicates by `seq` and treats an event as a refresh signal, never
// applying its `task` or `states` payload over a REST snapshot (ADR 0022).

export type TaskEventKind =
  | "created"
  | "updated"
  | "state_changed"
  | "claimed"
  | "released"
  | "escalated"
  | "blocked"
  | "unblocked"
  | "commented"
  | "dependency_added"
  | "dependency_removed"
  | "deleted"
  | "states_changed";

export type TaskActor =
  | { kind: "user"; user_id: string }
  | { kind: "session"; session_id: string }
  | { kind: "system" };

export interface TaskEvent {
  /** Per-project, monotonic; the stream's `Last-Event-ID` and `?after=`. */
  seq: number;
  ts: string;
  /** The original task UUID, retained after deletion; null on `states_changed`. */
  task_id: string | null;
  actor: TaskActor;
  kind: TaskEventKind;
  /** The full task after the change; absent on `deleted` and `states_changed`. */
  task?: Task;
  /** On `commented`. */
  comment?: TaskComment;
  /** State names, on `state_changed` and `escalated`. */
  from?: string;
  to?: string;
  /**
   * On `released`: `given_back` | `session_ended` | `stalled` | `user`.
   * On `escalated`: free text.
   */
  reason?: string;
  /** On `states_changed`: the project's full state list after the change. */
  states?: TaskState[];
}
