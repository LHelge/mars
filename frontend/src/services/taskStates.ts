// `SPEC.md`, "Task states": the columns of a project's board. States are
// addressed by `name`, which is also what a task's `state` holds, so a rename
// through `updateTaskState` renames the column everywhere at once.

import type {
  CreateTaskStateInput,
  TaskState,
  UpdateTaskStateInput,
} from "../types";
import { apiDelete, apiGet, apiPost, apiPut, seg } from "./apiClient";

/** Ordered by `position`. */
export function listTaskStates(pid: string): Promise<TaskState[]> {
  return apiGet<TaskState[]>(`/projects/${seg(pid)}/task-states`);
}

/** `POST /projects/{pid}/task-states` → 201; 409 on a taken name. */
export function createTaskState(
  pid: string,
  input: CreateTaskStateInput,
): Promise<TaskState> {
  return apiPost<TaskState>(`/projects/${seg(pid)}/task-states`, input);
}

/** `PUT /projects/{pid}/task-states/{name}` — rename and reorder only. */
export function updateTaskState(
  pid: string,
  name: string,
  input: UpdateTaskStateInput,
): Promise<TaskState> {
  return apiPut<TaskState>(
    `/projects/${seg(pid)}/task-states/${seg(name)}`,
    input,
  );
}

/** `DELETE /projects/{pid}/task-states/{name}` → 204; 409 under the API's rules. */
export function deleteTaskState(pid: string, name: string): Promise<void> {
  return apiDelete(`/projects/${seg(pid)}/task-states/${seg(name)}`);
}
