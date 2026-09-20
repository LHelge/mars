// Mirrors `SPEC.md`, "Tasks (`/api/projects/{pid}/tasks`)" and "Code hand-offs and
// review"; `TaskDependencyKind` is the `task_dependency_kind` enum of `docs/data-model.md`.

export type TaskDependencyKind = "blocks" | "discovered_from" | "related";

export type ReviewStatus = "unreviewed" | "approved" | "changes_requested";

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

export interface Comment {
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
  priority: 0 | 1 | 2 | 3;
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
  comments: Comment[];
  handoffs: Handoff[];
  children: Task[];
  sessions: TaskSessionTouch[];
}
