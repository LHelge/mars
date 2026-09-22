// The query keys of the task board, drawer and states editor. They delegate to
// the central factory in `services/queryKeys.ts` rather than spelling arrays
// out again, so an invalidation from the board hits the very same keys the
// dashboard, the session view and the profile editor already hold. Every task
// and task-state key comes from here and the other keys a task module needs
// come straight from `services/queryKeys`; no module writes an inline key
// array.

import { queryKeys } from "../services/queryKeys";
import type { TaskPathRef } from "../services/tasks";

export const taskKeys = {
  /**
   * One project's tasks. `detail` extends this key, so invalidating it
   * refreshes the board snapshot and every open task detail at once.
   */
  all: (projectId: string) => queryKeys.tasks.project(projectId),
  /** One task, by its per-project number (the URL route) or its UUID. */
  detail: (projectId: string, ref: TaskPathRef) =>
    queryKeys.tasks.detail(projectId, ref),
};

export const taskStateKeys = {
  /** The project's board columns, in `position` order. */
  list: (projectId: string) => queryKeys.projects.taskStates(projectId),
};
