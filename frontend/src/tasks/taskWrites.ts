// The task API's own verbs — update, release, delete and the dependency edges
// — one hook each (`SPEC.md`, "Tasks"), and the settle rule every task write
// shares. A write with a request of its own — a comment, a merge, a launch —
// makes it where it lives and settles through `useSettleTask` here.
//
// Each hook returns the request itself — an async function that writes and
// then settles the caches — and nothing else. Who owns the pending and error
// state of a submission is the caller's business and always a single owner
// (`CLAUDE.md`, "Frontend conventions", "Submitting a form"): a form wraps the
// request in `useFormSubmit`, a control that is not a form wraps it in a
// TanStack `useMutation` and reads `isPending` and `error` off that one.
//
// All of them settle the same way (`SPEC.md`, "Frontend", "Board refresh
// ordering"): the task's own detail query is invalidated so the drawer shows
// what the API just returned, and the board store is invalidated so the card
// behind the drawer follows without waiting for the SSE event. The event's own
// refresh follows a moment later: both are whole snapshots and both are shown,
// and the two are one read only when the event lands while the first refresh
// is still in flight (`SPEC.md`, "Frontend", "Board refresh ordering";
// ADR 0022).
//
// A failure invalidates the detail alone: a 409 from a concurrent change, or a
// 404 from a task someone else deleted mid-edit, is answered by re-reading the
// task — which is what turns the drawer into its not-found state — while the
// board is left to the stream.

import { useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  addDependency as addDependencyRequest,
  deleteTask,
  releaseTask,
  removeDependency as removeDependencyRequest,
  updateTask,
} from "../services/tasks";
import type { Task, TaskDependencyKind, UpdateTaskInput } from "../types";
import { taskKeys } from "./queryKeys";
import { useTaskStore } from "./taskStore";

/** One edge, as the add and remove controls name it. */
export interface DependencyEdge {
  /** The other task's UUID; the API also accepts its number. */
  depends_on: string;
  kind: TaskDependencyKind;
}

/**
 * What a task write leaves behind: the drawer rereads the task, and the board
 * behind it refreshes.
 *
 * The one settle rule of a task write, in one place — every caller that writes
 * a task awaits this rather than spelling the two invalidations out again.
 *
 * `number` is `null` for a write that names no task at all — the project
 * page's launch form before a task is typed into it — where the board is still
 * what goes stale and there is no drawer copy to reread.
 */
export function useSettleTask(
  projectId: string,
  number: number | null,
): () => Promise<void> {
  const refetchTask = useRefetchTask(projectId, number);

  return useCallback(async () => {
    await refetchTask();
    useTaskStore.getState().invalidate();
  }, [refetchTask]);
}

/**
 * The drawer's own read of the task, and nothing else: the answer to a refusal
 * that means what is on screen is already out of date, where the board has the
 * stream to tell it the same thing.
 *
 * With `null` there is no such copy — nothing on screen claims to be that
 * task — and this does nothing.
 */
export function useRefetchTask(
  projectId: string,
  number: number | null,
): () => Promise<void> {
  const queryClient = useQueryClient();

  return useCallback(
    () =>
      number === null
        ? Promise.resolve()
        : queryClient.invalidateQueries({
            queryKey: taskKeys.detail(projectId, number),
          }),
    [queryClient, projectId, number],
  );
}

/**
 * The settle-on-success, reread-on-failure wrapper every verb below shares.
 * The failure is rethrown: whoever owns the submission is the one that turns
 * it into a sentence.
 */
function useSettledWrite<TArgs extends unknown[], TResult>(
  request: (...args: TArgs) => Promise<TResult>,
  projectId: string,
  number: number,
): (...args: TArgs) => Promise<TResult> {
  const settle = useSettleTask(projectId, number);
  const refetchTask = useRefetchTask(projectId, number);

  return useCallback(
    async (...args: TArgs) => {
      let result: TResult;
      try {
        result = await request(...args);
      } catch (caught) {
        await refetchTask();
        throw caught;
      }
      await settle();
      return result;
    },
    [request, settle, refetchTask],
  );
}

/** `PUT`: the field edits, the state move and the hand-offs all use this one. */
export function useUpdateTask(
  projectId: string,
  number: number,
): (input: UpdateTaskInput) => Promise<Task> {
  const request = useCallback(
    (input: UpdateTaskInput) => updateTask(projectId, number, input),
    [projectId, number],
  );
  return useSettledWrite(request, projectId, number);
}

/** `POST .../release`: 409 when nobody holds the task. */
export function useReleaseTask(
  projectId: string,
  number: number,
): () => Promise<Task> {
  const request = useCallback(
    () => releaseTask(projectId, number),
    [projectId, number],
  );
  return useSettledWrite(request, projectId, number);
}

/**
 * `DELETE`: 204, after which the drawer has nothing left to show.
 *
 * The one write that does not reread the task when it works: there is nothing
 * left to read, and invalidating its detail would spend a guaranteed 404 on
 * the way back to the board. The board is what is left to refresh. A failed
 * delete rereads it like every other verb.
 */
export function useDeleteTask(
  projectId: string,
  number: number,
): () => Promise<void> {
  const refetchTask = useRefetchTask(projectId, number);

  return useCallback(async () => {
    try {
      await deleteTask(projectId, number);
    } catch (caught) {
      // A refusal leaves the task there, and a 404 says someone else deleted
      // it: either way the drawer's copy is the thing to read again.
      await refetchTask();
      throw caught;
    }
    useTaskStore.getState().invalidate();
  }, [projectId, number, refetchTask]);
}

/** `POST .../dependencies`: 409 when a `blocks` edge would close a cycle. */
export function useAddDependency(
  projectId: string,
  number: number,
): (edge: DependencyEdge) => Promise<Task> {
  const request = useCallback(
    (edge: DependencyEdge) => addDependencyRequest(projectId, number, edge),
    [projectId, number],
  );
  return useSettledWrite(request, projectId, number);
}

/** `DELETE .../dependencies/{dep}?kind=`: only that kind is removed. */
export function useRemoveDependency(
  projectId: string,
  number: number,
): (edge: DependencyEdge) => Promise<Task> {
  const request = useCallback(
    (edge: DependencyEdge) =>
      removeDependencyRequest(projectId, number, edge.depends_on, edge.kind),
    [projectId, number],
  );
  return useSettledWrite(request, projectId, number);
}
