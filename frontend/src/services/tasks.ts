// `SPEC.md`, "Tasks": the cross-project escalation list the dashboard reads and
// every project-scoped endpoint of the task board. A task reference in a path
// is the per-project `number` in UI calls; the UUID is accepted too.

import type {
  Comment,
  CreateTaskInput,
  Task,
  TaskDependencyKind,
  TaskDetail,
  TaskPriority,
  UpdateTaskInput,
} from "../types";
import { apiDelete, apiGet, apiPost, apiPut } from "./apiClient";

/** A task's UUID or its per-project number, as `{id}` in the paths below. */
export type TaskRef = string | number;

/** `?state=&label=&priority=&parent=&held=`; an omitted filter is not sent. */
export interface TaskFilters {
  state?: string;
  label?: string;
  priority?: TaskPriority;
  /** A parent task's UUID or number. */
  parent?: string;
  /** `true`: only leased tasks; `false`: only unleased ones. */
  held?: boolean;
}

function withQuery(path: string, params: URLSearchParams): string {
  const query = params.toString();
  return query.length > 0 ? `${path}?${query}` : path;
}

/**
 * `GET /tasks?state_kind=human` — every task sitting in its project's human
 * state, across all projects. Tasks from different projects can share a
 * `number`, so a caller keys and links by `project_id` too.
 */
export function listHumanTasks(): Promise<Task[]> {
  return apiGet<Task[]>("/tasks?state_kind=human");
}

/** `GET /projects/{pid}/tasks` — ordered by priority, then number. */
export function listTasks(pid: string, filters?: TaskFilters): Promise<Task[]> {
  const params = new URLSearchParams();
  if (filters?.state !== undefined) {
    params.set("state", filters.state);
  }
  if (filters?.label !== undefined) {
    params.set("label", filters.label);
  }
  if (filters?.priority !== undefined) {
    params.set("priority", String(filters.priority));
  }
  if (filters?.parent !== undefined) {
    params.set("parent", filters.parent);
  }
  if (filters?.held !== undefined) {
    params.set("held", String(filters.held));
  }
  return apiGet<Task[]>(withQuery(`/projects/${pid}/tasks`, params));
}

/** `POST /projects/{pid}/tasks` → 201. */
export function createTask(pid: string, input: CreateTaskInput): Promise<Task> {
  return apiPost<Task>(`/projects/${pid}/tasks`, input);
}

/**
 * `GET /projects/{pid}/tasks/{id}` — one task with its comments, hand-offs,
 * children and touching sessions. `idOrNumber` is the task's UUID or its
 * per-project number.
 */
export function getTask(pid: string, idOrNumber: TaskRef): Promise<TaskDetail> {
  return apiGet<TaskDetail>(`/projects/${pid}/tasks/${idOrNumber}`);
}

/** `PUT /projects/{pid}/tasks/{id}` — the state move and hand-off path too. */
export function updateTask(
  pid: string,
  ref: TaskRef,
  input: UpdateTaskInput,
): Promise<Task> {
  return apiPut<Task>(`/projects/${pid}/tasks/${ref}`, input);
}

/** `DELETE /projects/{pid}/tasks/{id}` → 204; children survive as top-level. */
export function deleteTask(pid: string, ref: TaskRef): Promise<void> {
  return apiDelete(`/projects/${pid}/tasks/${ref}`);
}

/**
 * `POST /projects/{pid}/tasks/{id}/dependencies` — `kind` defaults to `blocks`
 * server-side; 409 when a `blocks` edge would close a cycle.
 */
export function addDependency(
  pid: string,
  ref: TaskRef,
  input: { depends_on: string; kind?: TaskDependencyKind },
): Promise<Task> {
  return apiPost<Task>(`/projects/${pid}/tasks/${ref}/dependencies`, input);
}

/**
 * `DELETE /projects/{pid}/tasks/{id}/dependencies/{dep}?kind=` → the task.
 * `kind` is always sent: the API removes only edges of that kind, and the same
 * pair may carry several.
 */
export function removeDependency(
  pid: string,
  ref: TaskRef,
  dep: TaskRef,
  kind: TaskDependencyKind,
): Promise<Task> {
  const params = new URLSearchParams({ kind });
  return apiDelete<Task>(
    withQuery(`/projects/${pid}/tasks/${ref}/dependencies/${dep}`, params),
  );
}

/** `POST /projects/{pid}/tasks/{id}/comments` → 201. */
export function addComment(
  pid: string,
  ref: TaskRef,
  body: string,
): Promise<Comment> {
  return apiPost<Comment>(`/projects/${pid}/tasks/${ref}/comments`, { body });
}

/**
 * `POST /projects/{pid}/tasks/{id}/release` — clears the lease and keeps the
 * state; a user release never escalates. 409 when nobody holds it.
 */
export function releaseTask(pid: string, ref: TaskRef): Promise<Task> {
  return apiPost<Task>(`/projects/${pid}/tasks/${ref}/release`);
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
  return `/api/projects/${pid}/tasks/stream?token=${encodeURIComponent(token)}&after=${from}`;
}
