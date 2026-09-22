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
  created_at: string;
}

/** `POST /projects/{pid}/task-states`. */
export interface CreateTaskStateInput {
  name: string;
  kind: TaskStateKind;
  /** Omitted appends; an explicit one shifts the states at and after it. */
  position?: number;
}

/** `PUT /projects/{pid}/task-states/{name}`; `kind` is immutable. */
export interface UpdateTaskStateInput {
  name?: string;
  position?: number;
}
