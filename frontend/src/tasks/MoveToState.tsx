// Moving a task between the project's states from the drawer (`SPEC.md`,
// "Tasks": a user may set any state through `PUT` and is not bound by leases;
// `ARCHITECTURE.md`, "Task tracker", "The lease is the worker").
//
// Two things about the contract shape this control. Moving to a *different*
// state hands the task off: the lease is cleared and `attempts` reset. That is
// worth saying out loud before it happens when a session is holding the task,
// so the move asks for a confirmation that names the holder. Moving to the
// state the task is already in is a no-op that deliberately preserves the
// lease and the counters, so that option is offered as the current one and
// cannot be chosen.
//
// This is a planning move and carries no `handoff` (`SPEC.md`, "Code hand-offs
// and review": a planning-only state move needs no hand-off and preserves the
// existing one). Publishing work belongs to the hand-off controls.
//
// The control stores only the choice the user made, never the task's state.
// The drawer rereads the task on every task event ("Board refresh ordering"),
// so a target seeded from `task.state` would go on naming a state the task has
// since left: an agent hands a `ready` task to `review`, and the select still
// reads `ready` with Move armed, one confirmation away from sending the task
// back and clearing its lease. With `null` meaning "the user chose nothing",
// the target follows the task until somebody picks a state, and Move is armed
// only by that pick.

import { useState } from "react";
import { useMutation } from "@tanstack/react-query";

import { Alert } from "../components/Alert";
import { SubmitButton } from "../components/SubmitButton";
import { errorMessage } from "../services/errorMessage";
import type { Task } from "../types";
import { shortId } from "../utils/format";
import { CONTROL } from "../components/fieldStyles";
import { useDrawerEscape } from "./drawerEscape";
import { useTaskStore } from "./taskStore";
import { useUpdateTask } from "./taskWrites";

export interface MoveToStateProps {
  projectId: string;
  task: Task;
}

export function MoveToState({ projectId, task }: MoveToStateProps) {
  const states = useTaskStore((state) => state.states);
  // Not a form: the one owner of this write's pending and refusal is the
  // mutation itself (`CLAUDE.md`, "Frontend conventions", "Submitting a form"),
  // and the next attempt clears both.
  const updateTask = useUpdateTask(projectId, task.number);
  const update = useMutation({ mutationFn: updateTask });
  // The user's explicit choice, or `null` while they have made none.
  const [choice, setChoice] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);

  // Escape withdraws the confirmation as `Cancel` does, and like `Cancel` is
  // shut while the move is in flight (`drawerEscape.ts`).
  useDrawerEscape(() => {
    setConfirming(false);
  }, confirming && !update.isPending);

  const target = choice ?? task.state;
  const selectId = `task-${String(task.number)}-move-to`;
  // True while nothing is chosen, and again when the task reaches the state
  // that was chosen on its own: there is nothing left to send either way.
  const unchanged = target === task.state;

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <label htmlFor={selectId} className="text-console-muted text-xs">
          Move to
        </label>
        <select
          id={selectId}
          name={selectId}
          value={target}
          disabled={update.isPending}
          onChange={(event) => {
            setChoice(event.target.value);
            setConfirming(false);
          }}
          className={`${CONTROL} py-1`}
        >
          {states.map((state) => (
            <option
              key={state.id}
              value={state.name}
              disabled={state.name === task.state}
            >
              {state.name}
              {state.name === task.state ? " (current)" : ""}
            </option>
          ))}
          {/* A state renamed while the drawer was open: the task still names
              the old one, and the select must be able to show it. */}
          {!states.some((state) => state.name === task.state) && (
            <option value={task.state} disabled>
              {task.state} (current)
            </option>
          )}
        </select>
        <SubmitButton
          type="button"
          variant="ghost"
          disabled={unchanged || confirming || update.isPending}
          onClick={() => {
            setConfirming(true);
          }}
        >
          Move
        </SubmitButton>
      </div>

      {confirming && !unchanged && (
        <div className="border-console-border bg-console-bg flex flex-col gap-2 rounded border p-2">
          <p className="text-console-text max-w-prose text-sm">
            {task.lease_holder_session_id === null
              ? `Move #${String(task.number)} to ${target}.`
              : `Moving hands the task off: the lease held by session ${shortId(task.lease_holder_session_id, 8)} is cleared and attempts reset.`}
          </p>
          <div className="flex justify-end gap-2">
            <SubmitButton
              type="button"
              variant="ghost"
              disabled={update.isPending}
              onClick={() => {
                setConfirming(false);
              }}
            >
              Cancel
            </SubmitButton>
            <SubmitButton
              type="button"
              loading={update.isPending}
              onClick={() => {
                update.mutate(
                  { state: target },
                  {
                    onSuccess: () => {
                      setConfirming(false);
                      // The move landed: the task's own state is the answer
                      // again, and the select follows it.
                      setChoice(null);
                    },
                  },
                );
              }}
            >
              Move to {target}
            </SubmitButton>
          </div>
        </div>
      )}

      {update.isError && (
        <Alert kind="error">{errorMessage(update.error)}</Alert>
      )}
    </div>
  );
}
