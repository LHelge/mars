// Mirrors `SPEC.md`, "Task states (`/api/projects/{pid}/task-states`)": the
// columns of a project's board, in `position` order. `kind` is the
// `task_state_kind` enum of `docs/data-model.md`, "Enums".

export type TaskStateKind = "queue" | "human" | "terminal";

export interface TaskState {
  id: string;
  project_id: string;
  /** 1–32 characters matching `[a-z0-9][a-z0-9_-]*`; tasks reference states by id. */
  name: string;
  kind: TaskStateKind;
  position: number;
  created_at: string;
}
