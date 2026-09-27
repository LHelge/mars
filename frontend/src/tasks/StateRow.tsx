// One column of the board, as a row of the states table: its position, its
// name, what the orchestrator may do with it, how many tasks are sitting in it
// and the edits it offers — rename, move, remove and, on a queue state, the
// auto-merge pair of `SPEC.md`, "Task states" (ADR 0045).
//
// The row has one line to answer in, so moving and removing share one owner of
// their pending and refusal state (`CLAUDE.md`, "Frontend conventions",
// "Submitting a form"): a refusal from the last action cannot outlive a later
// one that worked. The rename keeps its own, because it is a form with a field
// of its own, and leaving edit mode takes its answers with it. The auto-merge
// toggle and its conflict state are one more such form: a draft of the pair,
// saved together in one `PUT` (`autoMergeRules.ts`), whose answer an edit or a
// cancel drops.
//
// Positions are packed to `0..n` after every change, so a row's index is its
// `position` and moving it is "insert me at the neighbour's index".

import { useState } from "react";
import type { FormEvent } from "react";

import { Alert } from "../components/Alert";
import { ConfirmPanel } from "../components/ConfirmPanel";
import { FieldShell } from "../components/FieldShell";
import {
  CHECK,
  CHECK_LABEL,
  CONTROL,
  TOUCH_TEXT,
} from "../components/fieldStyles";
import { SubmitButton } from "../components/SubmitButton";
import { CELL, ROW, SPAN_CELL } from "../components/tableStyles";
import { useFormSubmit } from "../hooks/useFormSubmit";
import { ApiError } from "../services/apiClient";
import { deleteTaskState, updateTaskState } from "../services/taskStates";
import type { TaskState } from "../types";
import {
  autoMergeChanged,
  autoMergeMeaning,
  autoMergeRefusal,
  CONFLICT_STATE_HINT,
  conflictCandidates,
  NO_CONFLICT_TARGET,
  toAutoMergeDraft,
  toAutoMergeUpdate,
  toggleAutoMerge,
} from "./autoMergeRules";
import type { AutoMergeDraft } from "./autoMergeRules";
import {
  COUNTS_UNKNOWN,
  deletionReason,
  KIND_COLOUR,
  KIND_MEANING,
  STATE_COLUMNS,
  stateNameError,
} from "./taskStateRules";

/** One of the two writes a row makes outside its rename form. */
type RowAction = { kind: "move"; to: number } | { kind: "remove" };

export interface StateRowProps {
  projectId: string;
  state: TaskState;
  /** The row's place in the sorted list, which is also its `position`. */
  index: number;
  states: TaskState[];
  /** Undefined while the task list is unread; see `TaskStatesEditor`. */
  counts: ReadonlyMap<string, number> | undefined;
  afterMutation: () => Promise<void>;
}

export function StateRow({
  projectId,
  state,
  index,
  states,
  counts,
  afterMutation,
}: StateRowProps) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(state.name);
  const [draftError, setDraftError] = useState<string | null>(null);
  const [confirmingRemove, setConfirmingRemove] = useState(false);

  const rename = useFormSubmit(async (next: string) => {
    await updateTaskState(projectId, state.name, { name: next });
    setEditing(false);
    await afterMutation();
  });

  // The auto-merge pair as the user is editing it, or null while the row shows
  // what is stored: a save or a cancel drops it, so the next read of the
  // states list is what the row shows again.
  const [mergeDraft, setMergeDraft] = useState<AutoMergeDraft | null>(null);
  const merge = useFormSubmit(
    async (draft: AutoMergeDraft) => {
      await updateTaskState(projectId, state.name, toAutoMergeUpdate(draft));
      await afterMutation();
      setMergeDraft(null);
    },
    { mapError: autoMergeRefusal },
  );

  const [acting, setActing] = useState<RowAction["kind"] | null>(null);
  const act = useFormSubmit(async (action: RowAction) => {
    if (action.kind === "move") {
      await updateTaskState(projectId, state.name, { position: action.to });
      await afterMutation();
      return;
    }
    try {
      await deleteTaskState(projectId, state.name);
    } catch (caught) {
      // The refusal this page thought it had ruled out: someone moved a task
      // or changed the state list while it was open. Show what the server
      // said and read both lists again, so the row tells the truth next.
      if (caught instanceof ApiError && caught.status === 409) {
        await afterMutation();
      }
      throw caught;
    }
    await afterMutation();
  });

  function run(action: RowAction) {
    setActing(action.kind);
    void act.submit(action);
  }

  const busy = rename.loading || act.loading || merge.loading;

  const shownMerge = mergeDraft ?? toAutoMergeDraft(state);
  const mergeDirty = mergeDraft !== null && autoMergeChanged(mergeDraft, state);
  const candidates = conflictCandidates(state, states);
  // Nowhere to send a conflict, and nothing stored to turn off: the toggle
  // would only produce a request the server refuses.
  const mergeRefusal =
    candidates.length === 0 && !shownMerge.autoMerge
      ? NO_CONFLICT_TARGET
      : null;

  function editMerge(next: AutoMergeDraft) {
    // The last save's answer described the pair before this edit.
    merge.reset();
    setMergeDraft(next);
  }

  function cancelMerge() {
    merge.reset();
    setMergeDraft(null);
  }

  function submitMerge(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (mergeDraft === null || !mergeDirty) {
      return;
    }
    void merge.submit(mergeDraft);
  }
  const count = counts?.get(state.name) ?? 0;

  // No counts, no removal: the structural reasons are knowable without them,
  // but "nothing is in it" is not.
  const refusal =
    counts === undefined
      ? COUNTS_UNKNOWN
      : deletionReason(state, states, counts);

  function startRename() {
    setDraft(state.name);
    setDraftError(null);
    rename.reset();
    setEditing(true);
  }

  function cancelRename() {
    setEditing(false);
    // Nothing is being edited any more, so neither answer describes anything.
    setDraftError(null);
    rename.reset();
  }

  function submitRename(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const next = draft.trim();
    if (next === state.name) {
      cancelRename();
      return;
    }
    const invalid = stateNameError(next);
    setDraftError(invalid);
    if (invalid !== null) {
      return;
    }
    void rename.submit(next);
  }

  function onRemove() {
    setConfirmingRemove(false);
    run({ kind: "remove" });
  }

  return (
    <>
      <tr className={ROW}>
        <td className={`${CELL} text-console-muted font-mono text-xs`}>
          {index}
        </td>

        <td className={CELL}>
          {editing ? (
            <form
              onSubmit={submitRename}
              aria-label={`Rename ${state.name}`}
              className="flex flex-wrap items-center gap-1.5"
            >
              <input
                id={`task-state-rename-${state.id}`}
                name="name"
                value={draft}
                onChange={(event) => {
                  setDraft(event.target.value);
                  setDraftError(null);
                }}
                aria-label="New name"
                aria-invalid={draftError !== null ? true : undefined}
                autoComplete="off"
                autoFocus
                className={`border-console-border bg-console-bg text-console-text aria-invalid:border-state-failed w-40 rounded border px-2 py-1 font-mono text-xs ${TOUCH_TEXT}`}
              />
              <SubmitButton loading={rename.loading}>Save</SubmitButton>
              <SubmitButton
                type="button"
                variant="ghost"
                disabled={rename.loading}
                onClick={cancelRename}
              >
                Cancel
              </SubmitButton>
            </form>
          ) : (
            <span className="text-console-text font-mono text-xs">
              {state.name}
            </span>
          )}
        </td>

        <td className={CELL}>
          <span
            title={KIND_MEANING[state.kind]}
            className={`border-console-border bg-console-surface inline-flex items-center rounded border px-1.5 py-0.5 font-mono text-xs ${KIND_COLOUR[state.kind]}`}
          >
            {state.kind}
          </span>
        </td>

        <td className={CELL}>
          {state.kind !== "queue" ? (
            <span
              className="text-console-muted font-mono text-xs"
              title="Only a queue state can merge automatically"
            >
              —
            </span>
          ) : (
            <form
              onSubmit={submitMerge}
              aria-label={`Auto-merge ${state.name}`}
              className="flex flex-wrap items-end gap-2"
            >
              <label
                className={`text-console-text flex items-center gap-1.5 self-center font-mono text-xs ${CHECK_LABEL}`}
                title={mergeRefusal ?? autoMergeMeaning(state.conflict_state)}
              >
                <input
                  type="checkbox"
                  name={`task-state-auto-merge-${state.id}`}
                  checked={shownMerge.autoMerge}
                  onChange={(event) => {
                    editMerge(
                      toggleAutoMerge(
                        shownMerge,
                        event.target.checked,
                        state,
                        states,
                      ),
                    );
                  }}
                  disabled={busy || mergeRefusal !== null}
                  className={CHECK}
                />
                Auto-merge
              </label>

              {shownMerge.autoMerge && (
                <FieldShell
                  label="Conflict state"
                  name={`task-state-conflict-${state.id}`}
                  hint={CONFLICT_STATE_HINT}
                  help="task-flow"
                >
                  {(control) => (
                    <select
                      {...control}
                      value={shownMerge.conflictState ?? ""}
                      onChange={(event) => {
                        editMerge({
                          autoMerge: true,
                          conflictState: event.target.value,
                        });
                      }}
                      disabled={busy}
                      className={CONTROL}
                    >
                      {candidates.map((candidate) => (
                        <option key={candidate.id} value={candidate.name}>
                          {candidate.name}
                        </option>
                      ))}
                    </select>
                  )}
                </FieldShell>
              )}

              {mergeDirty && (
                <div className="flex items-center gap-1.5 self-end">
                  <SubmitButton loading={merge.loading}>Save</SubmitButton>
                  <SubmitButton
                    type="button"
                    variant="ghost"
                    disabled={merge.loading}
                    onClick={cancelMerge}
                  >
                    Cancel
                  </SubmitButton>
                </div>
              )}
            </form>
          )}
        </td>

        <td
          className={`${CELL} text-console-muted text-right font-mono text-xs`}
          title={counts === undefined ? COUNTS_UNKNOWN : undefined}
        >
          {counts === undefined ? "—" : count}
        </td>

        <td className={`${CELL} pr-0`}>
          <div className="flex flex-wrap items-center justify-end gap-1.5">
            <SubmitButton
              type="button"
              variant="ghost"
              disabled={busy || index === 0}
              onClick={() => {
                run({ kind: "move", to: index - 1 });
              }}
            >
              <span aria-hidden="true">↑</span>
              <span className="sr-only">Move {state.name} up</span>
            </SubmitButton>

            <SubmitButton
              type="button"
              variant="ghost"
              disabled={busy || index === states.length - 1}
              onClick={() => {
                run({ kind: "move", to: index + 1 });
              }}
            >
              <span aria-hidden="true">↓</span>
              <span className="sr-only">Move {state.name} down</span>
            </SubmitButton>

            {!editing && (
              <SubmitButton
                type="button"
                variant="ghost"
                disabled={busy}
                onClick={startRename}
              >
                Rename
              </SubmitButton>
            )}

            {refusal !== null && (
              <span className="text-console-muted hidden text-xs md:inline">
                {refusal}
              </span>
            )}
            <span title={refusal ?? undefined}>
              <SubmitButton
                type="button"
                variant="danger"
                loading={act.loading && acting === "remove"}
                disabled={busy || refusal !== null || confirmingRemove}
                onClick={() => {
                  setConfirmingRemove(true);
                }}
              >
                Remove
              </SubmitButton>
            </span>
          </div>
        </td>
      </tr>

      {confirmingRemove && (
        <tr className={ROW}>
          <td colSpan={STATE_COLUMNS.length} className={SPAN_CELL}>
            <ConfirmPanel
              message={`Remove ${state.name}? The board loses the column, and agents stop seeing it as a place work can be.`}
              confirmLabel={`Remove ${state.name}`}
              pending={act.loading && acting === "remove"}
              onConfirm={onRemove}
              onCancel={() => {
                setConfirmingRemove(false);
              }}
            />
          </td>
        </tr>
      )}

      {/* The rename's answers belong to the rename: leaving edit mode takes
          them with it, rather than leaving a refusal under a row nobody is
          editing. */}
      {editing && (draftError !== null || rename.error !== null) && (
        <tr className={ROW}>
          <td colSpan={STATE_COLUMNS.length} className={SPAN_CELL}>
            <Alert kind="error">{draftError ?? rename.error}</Alert>
          </td>
        </tr>
      )}

      {/* The auto-merge save's answer, dropped with the draft it answered. */}
      {merge.error !== null && (
        <tr className={ROW}>
          <td colSpan={STATE_COLUMNS.length} className={SPAN_CELL}>
            <Alert kind="error">{merge.error}</Alert>
          </td>
        </tr>
      )}

      {act.error !== null && (
        <tr className={ROW}>
          <td colSpan={STATE_COLUMNS.length} className={SPAN_CELL}>
            <Alert kind="error">{act.error}</Alert>
          </td>
        </tr>
      )}
    </>
  );
}
