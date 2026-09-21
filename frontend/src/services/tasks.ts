// `SPEC.md`, "Tasks": the cross-project escalation list the dashboard reads and
// every project-scoped endpoint of the task board. A task reference in a path
// is the per-project `number` in UI calls; the UUID is accepted too.

import type {
  CreateTaskInput,
  Task,
  TaskComment,
  TaskDependencyKind,
  TaskDetail,
  UpdateTaskInput,
} from "../types";
import { apiDelete, apiGet, apiPost, apiPut, seg } from "./apiClient";

/** A task's UUID or its per-project number, as `{id}` in the paths below. */
export type TaskPathRef = string | number;

/**
 * `GET /tasks?state_kind=human` — every task sitting in its project's human
 * state, across all projects. Tasks from different projects can share a
 * `number`, so a caller keys and links by `project_id` too.
 */
export function listHumanTasks(): Promise<Task[]> {
  return apiGet<Task[]>("/tasks", { query: { state_kind: "human" } });
}

/**
 * `GET /projects/{pid}/tasks` — ordered by priority, then number.
 *
 * The endpoint's `?state=&label=&priority=&parent=&held=` filters are not
 * spelled here: the board reads the whole project once and filters the
 * snapshot in the browser (`SPEC.md`, "Frontend", "Task board"), so a filter
 * argument would be a parameter no caller ever passes.
 */
export function listTasks(pid: string): Promise<Task[]> {
  return apiGet<Task[]>(`/projects/${seg(pid)}/tasks`);
}

/** `POST /projects/{pid}/tasks` → 201. */
export function createTask(pid: string, input: CreateTaskInput): Promise<Task> {
  return apiPost<Task>(`/projects/${seg(pid)}/tasks`, input);
}

/**
 * `GET /projects/{pid}/tasks/{id}` — one task with its comments, hand-offs,
 * children and touching sessions. `idOrNumber` is the task's UUID or its
 * per-project number.
 */
export function getTask(
  pid: string,
  idOrNumber: TaskPathRef,
): Promise<TaskDetail> {
  return apiGet<TaskDetail>(`/projects/${seg(pid)}/tasks/${seg(idOrNumber)}`);
}

/** `PUT /projects/{pid}/tasks/{id}` — the state move and hand-off path too. */
export function updateTask(
  pid: string,
  ref: TaskPathRef,
  input: UpdateTaskInput,
): Promise<Task> {
  return apiPut<Task>(`/projects/${seg(pid)}/tasks/${seg(ref)}`, input);
}

/** `DELETE /projects/{pid}/tasks/{id}` → 204; children survive as top-level. */
export function deleteTask(pid: string, ref: TaskPathRef): Promise<void> {
  return apiDelete(`/projects/${seg(pid)}/tasks/${seg(ref)}`);
}

/**
 * `POST /projects/{pid}/tasks/{id}/dependencies` — `kind` defaults to `blocks`
 * server-side; 409 when a `blocks` edge would close a cycle.
 */
export function addDependency(
  pid: string,
  ref: TaskPathRef,
  input: { depends_on: string; kind?: TaskDependencyKind },
): Promise<Task> {
  return apiPost<Task>(
    `/projects/${seg(pid)}/tasks/${seg(ref)}/dependencies`,
    input,
  );
}

/**
 * `DELETE /projects/{pid}/tasks/{id}/dependencies/{dep}?kind=` → the task.
 * `kind` is always sent: the API removes only edges of that kind, and the same
 * pair may carry several.
 */
export function removeDependency(
  pid: string,
  ref: TaskPathRef,
  dep: TaskPathRef,
  kind: TaskDependencyKind,
): Promise<Task> {
  return apiDelete<Task>(
    `/projects/${seg(pid)}/tasks/${seg(ref)}/dependencies/${seg(dep)}`,
    { query: { kind } },
  );
}

/** `POST /projects/{pid}/tasks/{id}/comments` → 201. */
export function addComment(
  pid: string,
  ref: TaskPathRef,
  body: string,
): Promise<TaskComment> {
  return apiPost<TaskComment>(
    `/projects/${seg(pid)}/tasks/${seg(ref)}/comments`,
    { body },
  );
}

/**
 * `POST /projects/{pid}/tasks/{id}/release` — clears the lease and keeps the
 * state; a user release never escalates. 409 when nobody holds it.
 */
export function releaseTask(pid: string, ref: TaskPathRef): Promise<Task> {
  return apiPost<Task>(`/projects/${seg(pid)}/tasks/${seg(ref)}/release`);
}

/**
 * The SSE URL of `SPEC.md`, "SSE: task stream". `EventSource` cannot send an
 * `Authorization` header, so the access token travels as `?token=`.
 *
 * `after` is the last sequence the board received. A board that has received
 * none — a first connection, and a reconnect before the first event — has no
 * cursor to resume from, and asks for `after=latest` rather than 0: the whole
 * of a project's history would be replayed into a client that is about to read
 * an authoritative REST snapshot anyway (`SPEC.md`, "Frontend", "Board refresh
 * ordering"), at one `JSON.parse`, one store write and one invalidation each.
 *
 * A pure function so the stream hook and its test build the same URL.
 */
export function taskStreamUrl(pid: string, token: string, after = 0): string {
  const from = after > 0 ? String(after) : "latest";
  return `/api/projects/${seg(pid)}/tasks/stream?token=${encodeURIComponent(token)}&after=${from}`;
}
