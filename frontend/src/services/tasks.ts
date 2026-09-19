// `SPEC.md`, "Tasks": the cross-project escalation list the dashboard reads.
// The task board extends this module with the project-scoped endpoints.

import type { Task } from "../types";
import { apiGet } from "./apiClient";

/**
 * `GET /tasks?state_kind=human` — every task sitting in its project's human
 * state, across all projects. Tasks from different projects can share a
 * `number`, so a caller keys and links by `project_id` too.
 */
export function listHumanTasks(): Promise<Task[]> {
  return apiGet<Task[]>("/tasks?state_kind=human");
}
