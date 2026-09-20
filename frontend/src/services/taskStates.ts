// `SPEC.md`, "Task states": the read-only listing the session and project views
// need. The task board epic owns the mutations of this module.

import type { TaskState } from "../types";
import { apiGet } from "./apiClient";

/** Ordered by `position`. */
export function listTaskStates(pid: string): Promise<TaskState[]> {
  return apiGet<TaskState[]>(`/projects/${pid}/task-states`);
}
