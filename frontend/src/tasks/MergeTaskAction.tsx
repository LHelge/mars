// Merging the task's approved hand-off into an integration head (`SPEC.md`,
// "Git": the task form of `MergeInput`; `SPEC.md`, "Frontend", "Hand-off
// controls"; `ARCHITECTURE.md`, "Task tracker", "Review approval").
//
// The control sits beside the review badge rather than in the drawer's action
// bar, because it is that badge being acted on: an approval names a commit,
// and this is the button that puts that commit on a branch. Disabled without
// one, with the reason on it — the server answers 409 for the same case, and
// nobody should have to send a request to learn it.
//
// The approval can lapse while the form is open, when a new revision arrives
// unreviewed. That 409 is shown in the server's words and the task is
// refetched, which re-disables the button under the answer.

import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import { Alert } from "../components/Alert";
import { FieldShell } from "../components/FieldShell";
import { FIELD } from "../components/fieldStyles";
import { ConflictList } from "../components/git/ConflictList";
import { chosenOr, refsOfKind } from "../components/git/formState";
import { SubmitButton } from "../components/SubmitButton";
import { useFormSubmit } from "../hooks/useFormSubmit";
import { merge } from "../services/git";
import { getProject, listBranches } from "../services/projects";
import { queryKeys } from "../services/queryKeys";
import type { Branch, Handoff, TaskDetail } from "../types";
import { taskKeys } from "./queryKeys";
import {
  MERGE_ACTION,
  MERGE_BLOCKED,
  canMerge,
  isStaleMerge,
  mergeConflict,
  mergeCoverLine,
  mergeErrorMessage,
  mergedMessage,
} from "./mergeRules";
import type { MergeConflict } from "./mergeRules";
import { useTaskStore } from "./taskStore";

export interface MergeTaskActionProps {
  projectId: string;
  task: TaskDetail;
}

export function MergeTaskAction({ projectId, task }: MergeTaskActionProps) {
  const [open, setOpen] = useState(false);
  const allowed = canMerge(task);
  const handoff = task.handoff;

  return (
    <>
      {/* Compact, like the row it sits in: the summary line is one line of
          mono, and a form-sized button would break it. */}
      <span title={allowed ? undefined : MERGE_BLOCKED}>
        <button
          type="button"
          disabled={!allowed || open}
          onClick={() => {
            setOpen(true);
          }}
          className="border-console-border hover:bg-console-raised hover:text-console-text rounded border px-1.5 py-px font-mono text-[0.6875rem] disabled:opacity-50 disabled:hover:bg-transparent"
        >
          {MERGE_ACTION}
        </button>
      </span>

      {open && handoff !== null && (
        <MergeHandoffForm
          projectId={projectId}
          task={task}
          handoff={handoff}
          onClose={() => {
            setOpen(false);
          }}
        />
      )}
    </>
  );
}

function MergeHandoffForm({
  projectId,
  task,
  handoff,
  onClose,
}: {
  projectId: string;
  task: TaskDetail;
  handoff: Handoff;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();

  const project = useQuery({
    queryKey: queryKeys.projects.detail(projectId),
    queryFn: () => getProject(projectId),
  });
  const branches = useQuery({
    queryKey: queryKeys.projects.branches(projectId),
    queryFn: () => listBranches(projectId),
  });

  const heads: Branch[] = refsOfKind(branches.data ?? [], "head");
  const [chosenTarget, setChosenTarget] = useState("");
  // The project's `default_branch` is only a name until the mirror confirms
  // it; `chosenOr` falls back to a head that really exists.
  const target = chosenOr(
    chosenTarget === "" ? (project.data?.default_branch ?? "") : chosenTarget,
    heads,
  );

  const [message, setMessage] = useState("");
  const [merged, setMerged] = useState<string | null>(null);
  const [conflict, setConflict] = useState<MergeConflict | null>(null);
  const [refused, setRefused] = useState<string | null>(null);

  const form = useFormSubmit(async () => {
    setMerged(null);
    setConflict(null);
    setRefused(null);
    try {
      const result = await merge(projectId, {
        target,
        task_id: task.id,
        handoff_id: handoff.id,
        ...(message.trim() === "" ? {} : { message: message.trim() }),
      });
      setMerged(result.commit);
      // The merge moved an integration head and left the task where it was,
      // so the board's snapshot and the branch list are what went stale.
      void queryClient.invalidateQueries({
        queryKey: taskKeys.detail(projectId, task.number),
      });
      void queryClient.invalidateQueries({
        queryKey: queryKeys.projects.branches(projectId),
      });
      useTaskStore.getState().invalidate();
    } catch (caught) {
      const conflicts = mergeConflict(caught);
      if (conflicts !== null) {
        setConflict(conflicts);
        return;
      }
      if (isStaleMerge(caught)) {
        setRefused(mergeErrorMessage(caught));
        void queryClient.invalidateQueries({
          queryKey: taskKeys.detail(projectId, task.number),
        });
        return;
      }
      throw caught;
    }
  });

  return (
    <form
      aria-label={MERGE_ACTION}
      onSubmit={(event) => {
        event.preventDefault();
        void form.submit();
      }}
      className="border-console-border bg-console-bg basis-full space-y-3 rounded border p-3"
    >
      <p className="text-console-muted text-xs">{mergeCoverLine(handoff)}</p>

      <FieldShell label="Target" name="merge-target">
        {(control) => (
          <select
            {...control}
            value={target}
            disabled={form.loading}
            onChange={(event) => {
              setChosenTarget(event.target.value);
            }}
            className={FIELD}
          >
            {heads.length === 0 && (
              <option value="">No integration head</option>
            )}
            {heads.map((head) => (
              <option key={head.name} value={head.name}>
                {head.name}
              </option>
            ))}
          </select>
        )}
      </FieldShell>

      <FieldShell
        label="Message"
        name="merge-message"
        hint="Left empty, the merge commit names the hand-off and its branch."
      >
        {(control) => (
          <input
            {...control}
            value={message}
            disabled={form.loading}
            onChange={(event) => {
              setMessage(event.target.value);
            }}
            className={FIELD}
          />
        )}
      </FieldShell>

      {conflict !== null && (
        <ConflictList paths={conflict.paths} message={conflict.message} />
      )}
      {refused !== null && <Alert kind="error">{refused}</Alert>}
      {form.error !== null && (
        <Alert kind="error" onDismiss={form.clearError}>
          {form.error}
        </Alert>
      )}

      <div className="flex flex-wrap items-center gap-2">
        <SubmitButton loading={form.loading} disabled={target === ""}>
          Merge
        </SubmitButton>
        <SubmitButton
          type="button"
          variant="ghost"
          disabled={form.loading}
          onClick={onClose}
        >
          {merged === null ? "Cancel" : "Close"}
        </SubmitButton>
        {merged !== null && (
          <span className="text-state-running font-mono text-xs">
            {mergedMessage(merged)}
          </span>
        )}
      </div>
    </form>
  );
}
