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
import { useState } from "react";
import { useNavigate, useSearchParams } from "react-router";

import { Alert } from "../components/Alert";
import { SubmitButton } from "../components/SubmitButton";
import { projectErrorMessage } from "../pages/project/messages";
import type { TaskDetail } from "../types";
import { MoveToState } from "./MoveToState";
import { useTaskMutations } from "./useTaskMutations";

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
  const { release, remove } = useTaskMutations(projectId, task.number);
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
          loading={false}
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

        {/* `LaunchForTask` — "open in session" and "run once" — mounts here. */}
        {/* The hand-off and review controls mount here. */}

        <div className="border-console-border ml-auto border-l pl-2">
          <SubmitButton
            type="button"
            variant="danger"
            loading={false}
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

      {release.isError && (
        <Alert kind="error">{projectErrorMessage(release.error)}</Alert>
      )}

      {confirmingDelete && (
        <div className="border-state-failed/60 bg-console-bg flex flex-col gap-2 rounded border p-2">
          <p className="text-console-text max-w-prose text-sm">
            Delete #{task.number} &ldquo;{task.title}&rdquo;? Dependants are
            unblocked and its hand-off refs removed.
          </p>
          {remove.isError && (
            <Alert kind="error">{projectErrorMessage(remove.error)}</Alert>
          )}
          <div className="flex justify-end gap-2">
            <SubmitButton
              type="button"
              variant="ghost"
              loading={false}
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
