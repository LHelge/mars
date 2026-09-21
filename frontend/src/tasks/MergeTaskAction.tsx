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
// unreviewed. The form merges the hand-off it was opened on — the id is taken
// when the button is pressed, not read off the task on every render — so a
// revision arriving mid-form cannot move the merge onto a commit nobody
// approved; the request carries the superseded id and the server answers 409.
// That 409 is shown in the server's words and the task is refetched, which
// re-disables the button under the answer. The form says the revision arrived
// as soon as it does, rather than at the refusal.
//
// The outcome of a merge is one thing: merged, conflicted or refused. It is
// held as one value for that reason — three flags for three exclusive
// outcomes can show two answers at once — and a merge that landed disarms its
// own button, because a second press would merge the same commit twice.

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
import { queryKeys } from "../services/queryKeys";
import { projectQueries } from "../services/queryOptions";
import type { Branch, Handoff, TaskDetail } from "../types";
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
import { useDrawerEscape } from "./drawerEscape";
import { useRefetchTask, useSettleTask } from "./taskWrites";

export interface MergeTaskActionProps {
  projectId: string;
  task: TaskDetail;
}

export function MergeTaskAction({ projectId, task }: MergeTaskActionProps) {
  // The hand-off the form was opened on, which is also whether it is open.
  const [opened, setOpened] = useState<Handoff | null>(null);
  const allowed = canMerge(task);
  const handoff = task.handoff;

  return (
    <>
      {/* Compact, like the row it sits in: the summary line is one line of
          mono, and a form-sized button would break it. */}
      <span title={allowed ? undefined : MERGE_BLOCKED}>
        <button
          type="button"
          disabled={!allowed || handoff === null || opened !== null}
          onClick={() => {
            setOpened(handoff);
          }}
          className="border-console-border hover:bg-console-raised hover:text-console-text rounded border px-1.5 py-px font-mono text-[0.6875rem] disabled:opacity-50 disabled:hover:bg-transparent"
        >
          {MERGE_ACTION}
        </button>
      </span>

      {opened !== null && (
        <MergeHandoffForm
          projectId={projectId}
          task={task}
          handoff={opened}
          onClose={() => {
            setOpened(null);
          }}
        />
      )}
    </>
  );
}

/** What one merge attempt ended as; the three outcomes exclude each other. */
type MergeOutcome =
  | { kind: "merged"; commit: string }
  | { kind: "conflict"; conflict: MergeConflict }
  | { kind: "refused"; message: string };

function MergeHandoffForm({
  projectId,
  task,
  handoff,
  onClose,
}: {
  projectId: string;
  task: TaskDetail;
  /** The hand-off this form was opened on; the merge names this one. */
  handoff: Handoff;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const settle = useSettleTask(projectId, task.number);
  const refetchTask = useRefetchTask(projectId, task.number);

  const project = useQuery(projectQueries.detail(projectId));
  const branches = useQuery(projectQueries.branches(projectId));

  const heads: Branch[] = refsOfKind(branches.data ?? [], "head");
  const [chosenTarget, setChosenTarget] = useState("");
  // The project's `default_branch` is only a name until the mirror confirms
  // it; `chosenOr` falls back to a head that really exists. With no heads at
  // all there is nothing to fall back to — `chosenOr` hands the name back
  // unchanged — so the target is empty, which is what the select shows and
  // what disables Merge, rather than a branch the server would answer 400 for.
  const target =
    heads.length === 0
      ? ""
      : chosenOr(
          chosenTarget === ""
            ? (project.data?.default_branch ?? "")
            : chosenTarget,
          heads,
        );

  const [message, setMessage] = useState("");
  const [outcome, setOutcome] = useState<MergeOutcome | null>(null);
  const superseded = task.handoff?.id !== handoff.id;

  const form = useFormSubmit(async () => {
    setOutcome(null);
    try {
      const result = await merge(projectId, {
        target,
        task_id: task.id,
        handoff_id: handoff.id,
        ...(message.trim() === "" ? {} : { message: message.trim() }),
      });
      setOutcome({ kind: "merged", commit: result.commit });
      // The merge moved an integration head and left the task where it was,
      // so the board's snapshot and the branch list are what went stale.
      await settle();
      void queryClient.invalidateQueries({
        queryKey: queryKeys.projects.branches(projectId),
      });
    } catch (caught) {
      const conflicts = mergeConflict(caught);
      if (conflicts !== null) {
        setOutcome({ kind: "conflict", conflict: conflicts });
        return;
      }
      if (isStaleMerge(caught)) {
        setOutcome({ kind: "refused", message: mergeErrorMessage(caught) });
        await refetchTask();
        return;
      }
      throw caught;
    }
  });

  // Escape leaves the form as `Cancel` does, and is shut while the merge is in
  // flight for the same reason it is (`drawerEscape.ts`).
  useDrawerEscape(onClose, !form.loading);

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

      {superseded && (
        <Alert kind="warning">
          A newer revision was published while this form was open. It merges the
          commit above, which the server will refuse now that it is no longer
          the task's current hand-off.
        </Alert>
      )}

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
              <option value="">
                {branches.isPending
                  ? "Loading branches…"
                  : "No integration head"}
              </option>
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

      {outcome?.kind === "conflict" && (
        <ConflictList
          paths={outcome.conflict.paths}
          message={outcome.conflict.message}
        />
      )}
      {outcome?.kind === "refused" && (
        <Alert kind="error">{outcome.message}</Alert>
      )}
      {form.error !== null && (
        <Alert kind="error" onDismiss={form.reset}>
          {form.error}
        </Alert>
      )}

      <div className="flex flex-wrap items-center gap-2">
        {/* Merged is final for this form: the commit is on the branch, and a
            second press would put it there again. */}
        <SubmitButton
          loading={form.loading}
          disabled={target === "" || outcome?.kind === "merged"}
        >
          Merge
        </SubmitButton>
        <SubmitButton
          type="button"
          variant="ghost"
          disabled={form.loading}
          onClick={onClose}
        >
          {outcome?.kind === "merged" ? "Close" : "Cancel"}
        </SubmitButton>
        {outcome?.kind === "merged" && (
          <span className="text-state-running font-mono text-xs">
            {mergedMessage(outcome.commit)}
          </span>
        )}
      </div>
    </form>
  );
}
