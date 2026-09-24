// Mirrors `SPEC.md`, "Task states (`/api/projects/{pid}/task-states`)": the
// columns of a project's board, in `position` order. `kind` is the
// `task_state_kind` enum of `docs/data-model.md`, "Enums".

/** In the order the editor offers them. */
export const TASK_STATE_KINDS = ["queue", "human", "terminal"] as const;

export type TaskStateKind = (typeof TASK_STATE_KINDS)[number];

/** The kind a select's string is, or `undefined` for anything else. */
export function parseTaskStateKind(value: string): TaskStateKind | undefined {
  return TASK_STATE_KINDS.find((kind) => kind === value);
}

export interface TaskState {
  id: string;
  project_id: string;
  /**
   * 1–32 characters matching `[a-z0-9][a-z0-9_-]*`; a task's `state` is this
   * name, and a rename moves every task with it.
   */
  name: string;
  kind: TaskStateKind;
  position: number;
  /**
   * The orchestrator merges every unheld, unblocked task here whose current
   * hand-off is approved and moves it to the first terminal state
   * (`ARCHITECTURE.md`, "Task tracker" → "Automatic merges"). Queue states only.
   */
  auto_merge: boolean;
  /**
   * The name of the queue state a conflicting automatic merge sends the task
   * to; null exactly when `auto_merge` is false.
   */
  conflict_state: string | null;
  created_at: string;
}

/** `POST /projects/{pid}/task-states`. */
export interface CreateTaskStateInput {
  name: string;
  kind: TaskStateKind;
  /** Omitted appends; an explicit one shifts the states at and after it. */
  position?: number;
  auto_merge?: boolean;
  conflict_state?: string | null;
}

/**
 * `PUT /projects/{pid}/task-states/{name}`; `kind` is immutable. `auto_merge`
 * and `conflict_state` travel together: a body that gives either replaces
 * both, and one with neither leaves them alone.
 */
export interface UpdateTaskStateInput {
  name?: string;
  position?: number;
  auto_merge?: boolean;
  conflict_state?: string | null;
}
