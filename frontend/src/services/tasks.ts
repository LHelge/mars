// `SPEC.md`, "Tasks": the cross-project escalation list the dashboard reads.
// The task board extends this module with the project-scoped endpoints.

import type { Task, TaskDetail } from "../types";
import { apiGet } from "./apiClient";

/**
 * `GET /tasks?state_kind=human` — every task sitting in its project's human
 * state, across all projects. Tasks from different projects can share a
 * `number`, so a caller keys and links by `project_id` too.
 */
export function listHumanTasks(): Promise<Task[]> {
  return apiGet<Task[]>("/tasks?state_kind=human");
}

/**
 * `GET /projects/{pid}/tasks/{id}` — one task with its comments, hand-offs,
 * children and touching sessions. `idOrNumber` is the task's UUID or its
 * per-project number.
 */
export function getTask(
  pid: string,
  idOrNumber: string | number,
): Promise<TaskDetail> {
  return apiGet<TaskDetail>(`/projects/${pid}/tasks/${idOrNumber}`);
}
