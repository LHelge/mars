// The drawer's action bar: everything a user can do to a task that is not
// writing a comment (`SPEC.md`, "Frontend", "Task board": "actions: move to a
// state, release, open in session"; `ARCHITECTURE.md`, "Task tracker", "The
// lease is the worker": a user may move, release and edit anything).
//
// The bar is deliberately quiet — ghost buttons in a single row, the same
// weight as the metadata above them — because the drawer is for reading a task
// far more often than for changing one. Delete is the exception: it sits apart
// on the right, behind a rule, in the failed state's colour, and asks first.
//
// Each control owns its own pending state and its own refusal. A release in
// flight disables Release and nothing else, and a 409 is shown beside the
// button that caused it, in the API's own words.

import { PencilSquareIcon, TrashIcon } from "@heroicons/react/24/outline";
import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import { useNavigate, useSearchParams } from "react-router";

import { Alert } from "../components/Alert";
import { SubmitButton } from "../components/SubmitButton";
import { errorMessage } from "../services/errorMessage";
import type { TaskDetail } from "../types";
import { LaunchForTask } from "./LaunchForTask";
import { MoveToState } from "./MoveToState";
import { useDeleteTask, useReleaseTask } from "./taskWrites";

export interface TaskActionsProps {
  projectId: string;
  task: TaskDetail;
  /** The drawer is showing the edit form in place of the task's fields. */
  editing: boolean;
  onEditingChange: (editing: boolean) => void;
}

export function TaskActions({
  projectId,
  task,
  editing,
  onEditingChange,
}: TaskActionsProps) {
  const navigate = useNavigate();
  const [search] = useSearchParams();
  // Neither button is a form, so each write's mutation is its own single
  // owner of pending and refusal (`CLAUDE.md`, "Frontend conventions",
  // "Submitting a form"): a release in flight disables Release and nothing
  // else, and a retry clears what the last attempt said.
  const release = useMutation({
    mutationFn: useReleaseTask(projectId, task.number),
  });
  const remove = useMutation({
    mutationFn: useDeleteTask(projectId, task.number),
  });
  const [confirmingDelete, setConfirmingDelete] = useState(false);

  const held = task.lease_holder_session_id !== null;

  function backToBoard() {
    const params = new URLSearchParams(search);
    params.set("tab", "board");
    void navigate(`/projects/${projectId}?${params.toString()}`);
  }

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-2">
        <SubmitButton
          type="button"
          variant="ghost"
          disabled={editing}
          onClick={() => {
            onEditingChange(true);
          }}
        >
          <span className="flex items-center gap-1.5">
            <PencilSquareIcon aria-hidden="true" className="size-4" />
            Edit
          </span>
        </SubmitButton>

        <SubmitButton
          type="button"
          variant="ghost"
          loading={release.isPending}
          disabled={!held}
          onClick={() => {
            release.mutate();
          }}
        >
          Release
        </SubmitButton>

        {/* The hand-off and review controls are not here: they live in the
            drawer's "Code hand-off" section, beside the commit and review
            badge they talk about, where their forms have the body's width. */}

        <div className="border-console-border ml-auto border-l pl-2">
          <SubmitButton
            type="button"
            variant="danger"
            disabled={confirmingDelete || remove.isPending}
            onClick={() => {
              setConfirmingDelete(true);
            }}
          >
            <span className="flex items-center gap-1.5">
              <TrashIcon aria-hidden="true" className="size-4" />
              Delete
            </span>
          </SubmitButton>
        </div>
      </div>

      <MoveToState projectId={projectId} task={task} />

      {/* A row of its own: the launch form opens full-width beneath its
          two buttons, which the wrapping row above has no room for. */}
      <LaunchForTask projectId={projectId} task={task} />

      {release.isError && (
        <Alert kind="error">{errorMessage(release.error)}</Alert>
      )}

      {confirmingDelete && (
        <div className="border-state-failed/60 bg-console-bg flex flex-col gap-2 rounded border p-2">
          <p className="text-console-text max-w-prose text-sm">
            Delete #{task.number} &ldquo;{task.title}&rdquo;? Dependants are
            unblocked and its hand-off refs removed.
          </p>
          {remove.isError && (
            <Alert kind="error">{errorMessage(remove.error)}</Alert>
          )}
          <div className="flex justify-end gap-2">
            <SubmitButton
              type="button"
              variant="ghost"
              disabled={remove.isPending}
              onClick={() => {
                setConfirmingDelete(false);
              }}
            >
              Keep the task
            </SubmitButton>
            <SubmitButton
              type="button"
              variant="danger"
              loading={remove.isPending}
              onClick={() => {
                remove.mutate(undefined, { onSuccess: backToBoard });
              }}
            >
              Delete task
            </SubmitButton>
          </div>
        </div>
      )}
    </div>
  );
}
