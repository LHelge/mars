// Every write the task drawer makes, in one place (`SPEC.md`, "Tasks").
//
// All of them settle the same way (`SPEC.md`, "Frontend", "Board refresh
// ordering"): the task's own detail query is invalidated so the drawer shows
// what the API just returned, and the board store is invalidated so the card
// behind the drawer follows without waiting for the SSE event. The event
// arrives a moment later and is coalesced into the same refresh path, so a
// mutation never produces two reads (ADR 0022).
//
// A failure invalidates the detail alone: a 409 from a concurrent change, or a
// 404 from a task someone else deleted mid-edit, is answered by re-reading the
// task — which is what turns the drawer into its not-found state — while the
// board is left to the stream.
//
// Each control mounts its own copy of this hook. The mutations are then
// independent, which is exactly the rule the drawer wants: a pending action
// disables its own button and no other.

import { useCallback } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { UseMutationResult } from "@tanstack/react-query";

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

export interface TaskMutations {
  /** `PUT`: the field edits and the state move both go through this one. */
  update: UseMutationResult<Task, Error, UpdateTaskInput>;
  /** `POST .../release`: 409 when nobody holds the task. */
  release: UseMutationResult<Task, Error, void>;
  /** `DELETE`: 204, after which the drawer has nothing left to show. */
  remove: UseMutationResult<void, Error, void>;
  /** `POST .../dependencies`: 409 when a `blocks` edge would close a cycle. */
  addDependency: UseMutationResult<Task, Error, DependencyEdge>;
  /** `DELETE .../dependencies/{dep}?kind=`: only that kind is removed. */
  removeDependency: UseMutationResult<Task, Error, DependencyEdge>;
  /** Some write is in flight; the drawer dims what it cannot answer for. */
  pending: boolean;
}

export function useTaskMutations(
  projectId: string,
  number: number,
): TaskMutations {
  const queryClient = useQueryClient();

  const refetchTask = useCallback(() => {
    void queryClient.invalidateQueries({
      queryKey: taskKeys.detail(projectId, number),
    });
  }, [queryClient, projectId, number]);

  const settle = useCallback(() => {
    refetchTask();
    useTaskStore.getState().invalidate();
  }, [refetchTask]);

  const update = useMutation({
    mutationFn: (input: UpdateTaskInput) =>
      updateTask(projectId, number, input),
    onSuccess: settle,
    onError: refetchTask,
  });

  const release = useMutation({
    mutationFn: () => releaseTask(projectId, number),
    onSuccess: settle,
    onError: refetchTask,
  });

  const remove = useMutation({
    mutationFn: () => deleteTask(projectId, number),
    onSuccess: settle,
    onError: refetchTask,
  });

  const addDependency = useMutation({
    mutationFn: (edge: DependencyEdge) =>
      addDependencyRequest(projectId, number, edge),
    onSuccess: settle,
    onError: refetchTask,
  });

  const removeDependency = useMutation({
    mutationFn: (edge: DependencyEdge) =>
      removeDependencyRequest(projectId, number, edge.depends_on, edge.kind),
    onSuccess: settle,
    onError: refetchTask,
  });

  return {
    update,
    release,
    remove,
    addDependency,
    removeDependency,
    pending:
      update.isPending ||
      release.isPending ||
      remove.isPending ||
      addDependency.isPending ||
      removeDependency.isPending,
  };
}
