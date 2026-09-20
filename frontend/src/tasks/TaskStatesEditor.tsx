// The `states` tab of `/projects/:id` (`SPEC.md`, "Task states"): the columns
// of this project's board, in `position` order, and the four edits a user can
// make to them — add, rename, reorder, remove.
//
// The list is the board read as a table. Each row carries the one number that
// decides whether it can be removed at all — how many tasks are sitting in it
// — so the tasks are read alongside the states and grouped by name. That list
// is not paginated (`SPEC.md`, "Non-goals"), so the count is exact rather than
// a sample, and it refetches on window focus: a state the user just emptied in
// another tab should be removable when they come back to this one.
//
// Deletion refusals are shown before the request, not after it
// (`taskStateRules.ts`). Everything else is the server's answer, verbatim: a
// taken name, an invalid one, a second human state, and the 409 that still
// arrives when someone else moved a task while this page was open.
//
// Positions are packed to `0..n` after every change, so a state's `position`
// is its index in the list. Moving a row is therefore "insert me at the
// neighbour's index" and needs no arithmetic of its own.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useMemo, useState } from "react";
import type { FormEvent } from "react";
import { Alert } from "../components/Alert";
import { EmptyState } from "../components/EmptyState";
import { FormField } from "../components/FormField";
import { LoadingState } from "../components/LoadingState";
import { SectionHeader } from "../components/SectionHeader";
import { SubmitButton } from "../components/SubmitButton";
import { ApiError } from "../services/apiClient";
import { listTasks } from "../services/tasks";
import {
  createTaskState,
  deleteTaskState,
  listTaskStates,
  updateTaskState,
} from "../services/taskStates";
import type { TaskState, TaskStateKind } from "../types";
import { useFormSubmit } from "../hooks/useFormSubmit";
import { taskKeys, taskStateKeys } from "./queryKeys";
import {
  countTasksByState,
  deletionReason,
  HUMAN_TAKEN,
  KIND_COLOUR,
  KIND_MEANING,
  stateNameError,
} from "./taskStateRules";
import { useTaskStore } from "./taskStore";

/** `SPEC.md`, "Task states", in the one line the tab has room for. */
const STATES_HELP =
  "The columns of this project's board, in order. Agents claim work from a queue state, escalations land in the human state, and a terminal state closes the task.";

/** Shown instead of a count while the task list has not been read. */
const COUNTS_UNKNOWN = "Task counts are not loaded";

const KINDS: TaskStateKind[] = ["queue", "human", "terminal"];

const HEAD = "text-console-muted py-1.5 pr-3 text-left text-xs font-normal";
const CELL = "py-1.5 pr-3 align-middle";

export interface TaskStatesEditorProps {
  projectId: string;
}

export function TaskStatesEditor({ projectId }: TaskStatesEditorProps) {
  const queryClient = useQueryClient();

  const states = useQuery({
    queryKey: taskStateKeys.list(projectId),
    queryFn: () => listTaskStates(projectId),
    retry: false,
  });

  const tasks = useQuery({
    queryKey: taskKeys.all(projectId),
    queryFn: () => listTasks(projectId),
    retry: false,
    // This tab is not on the task stream; the board is. Coming back to the
    // window is the moment to find out that a state was emptied elsewhere.
    refetchOnWindowFocus: true,
  });

  /**
   * What every successful edit does. States and tasks are read together here,
   * and a task's `state` is the state's name, so a rename changes both lists
   * at once and neither may be left behind.
   *
   * This is also where the board store is told to refresh once it is bound to
   * this project — one place, so the two callers never drift apart.
   */
  const afterMutation = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({
        queryKey: taskStateKeys.list(projectId),
      }),
      queryClient.invalidateQueries({ queryKey: taskKeys.all(projectId) }),
    ]);
    const board = useTaskStore.getState();
    if (board.projectId === projectId) {
      board.invalidate();
    }
  }, [queryClient, projectId]);

  const rows = useMemo(
    () => [...(states.data ?? [])].sort((a, b) => a.position - b.position),
    [states.data],
  );

  // `undefined` while the task list is unread or failed: a count of zero would
  // enable a delete the server is about to refuse.
  const counts = useMemo(
    () =>
      tasks.data === undefined ? undefined : countTasksByState(tasks.data),
    [tasks.data],
  );

  const hasHuman = rows.some((state) => state.kind === "human");

  return (
    <section className="space-y-3">
      <SectionHeader title="Task states" description={STATES_HELP} />

      {states.isError && (
        <Alert kind="error">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span>{errorMessage(states.error)}</span>
            <SubmitButton
              type="button"
              variant="ghost"
              loading={states.isFetching}
              onClick={() => {
                void states.refetch();
              }}
            >
              Try again
            </SubmitButton>
          </div>
        </Alert>
      )}

      {tasks.isError && (
        <Alert kind="warning">
          {COUNTS_UNKNOWN}: {errorMessage(tasks.error)}. Removing a state waits
          until they are.
        </Alert>
      )}

      <AddStateForm
        projectId={projectId}
        hasHuman={hasHuman}
        stateCount={rows.length}
        afterMutation={afterMutation}
      />

      {states.isPending ? (
        <LoadingState label="Loading task states" />
      ) : rows.length === 0 ? (
        <EmptyState
          title="No task states"
          description="Add the first queue state above; the board has no columns until one exists."
        />
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="border-console-border border-b">
                <th scope="col" className={`${HEAD} w-8`}>
                  #
                </th>
                <th scope="col" className={HEAD}>
                  Name
                </th>
                <th scope="col" className={HEAD}>
                  Kind
                </th>
                <th scope="col" className={`${HEAD} text-right`}>
                  Tasks
                </th>
                <th scope="col" className={`${HEAD} pr-0 text-right`}>
                  Actions
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((state, index) => (
                <StateRow
                  key={state.id}
                  projectId={projectId}
                  state={state}
                  index={index}
                  states={rows}
                  counts={counts}
                  afterMutation={afterMutation}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

interface AddStateFormProps {
  projectId: string;
  hasHuman: boolean;
  stateCount: number;
  afterMutation: () => Promise<void>;
}

function AddStateForm({
  projectId,
  hasHuman,
  stateCount,
  afterMutation,
}: AddStateFormProps) {
  const [name, setName] = useState("");
  const [kind, setKind] = useState<TaskStateKind>("queue");
  const [position, setPosition] = useState("");
  const [nameError, setNameError] = useState<string | null>(null);
  const [positionError, setPositionError] = useState<string | null>(null);

  const add = useFormSubmit(async () => {
    await createTaskState(projectId, {
      name: name.trim(),
      kind,
      ...(position.trim() === "" ? {} : { position: Number(position) }),
    });
    await afterMutation();
    setName("");
    setKind("queue");
    setPosition("");
  });

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    const invalidName = stateNameError(name.trim());
    const trimmedPosition = position.trim();
    const invalidPosition =
      trimmedPosition !== "" && !/^\d+$/.test(trimmedPosition)
        ? "Position must be a whole number, counting from 0"
        : null;

    setNameError(invalidName);
    setPositionError(invalidPosition);
    if (invalidName !== null || invalidPosition !== null) {
      return;
    }

    void add.submit();
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label="Add a task state"
      className="border-console-border bg-console-surface flex flex-col gap-3 rounded border p-3"
    >
      <div className="grid gap-3 sm:grid-cols-[minmax(10rem,2fr)_minmax(8rem,1fr)_minmax(6rem,1fr)]">
        <FormField
          label="Name"
          name="task-state-name"
          value={name}
          onChange={(next) => {
            setName(next);
            setNameError(null);
          }}
          error={nameError ?? undefined}
          hint="What agents and the board call this column."
          autoComplete="off"
          required
        />

        <FormField
          label="Kind"
          name="task-state-kind"
          value={kind}
          onChange={(next) => {
            setKind(next as TaskStateKind);
          }}
          hint={KIND_MEANING[kind]}
        >
          <select
            id="task-state-kind"
            name="task-state-kind"
            value={kind}
            onChange={(event) => {
              setKind(event.target.value as TaskStateKind);
            }}
            aria-describedby="task-state-kind-hint"
            className="border-console-border bg-console-bg text-console-text rounded border px-2.5 py-1.5 font-mono text-sm"
          >
            {KINDS.map((option) => (
              <option
                key={option}
                value={option}
                disabled={option === "human" && hasHuman}
                title={option === "human" && hasHuman ? HUMAN_TAKEN : undefined}
              >
                {option}
                {option === "human" && hasHuman ? ` — ${HUMAN_TAKEN}` : ""}
              </option>
            ))}
          </select>
        </FormField>

        <FormField
          label="Position"
          name="task-state-position"
          type="number"
          value={position}
          onChange={(next) => {
            setPosition(next);
            setPositionError(null);
          }}
          error={positionError ?? undefined}
          hint={`Empty appends, at ${String(stateCount)}.`}
          autoComplete="off"
        />
      </div>

      {add.error !== null && <Alert kind="error">{add.error}</Alert>}

      <div className="flex justify-end">
        <SubmitButton loading={add.loading}>Add state</SubmitButton>
      </div>
    </form>
  );
}

interface StateRowProps {
  projectId: string;
  state: TaskState;
  /** The row's place in the sorted list, which is also its `position`. */
  index: number;
  states: TaskState[];
  /** Undefined while the task list is unread; see `TaskStatesEditor`. */
  counts: Record<string, number> | undefined;
  afterMutation: () => Promise<void>;
}

function StateRow({
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

  const rename = useMutation({
    mutationFn: (next: string) =>
      updateTaskState(projectId, state.name, { name: next }),
    onSuccess: async () => {
      setEditing(false);
      await afterMutation();
    },
  });

  const move = useMutation({
    mutationFn: (to: number) =>
      updateTaskState(projectId, state.name, { position: to }),
    onSuccess: () => afterMutation(),
  });

  const remove = useMutation({
    mutationFn: () => deleteTaskState(projectId, state.name),
    onSuccess: () => afterMutation(),
    onError: (caught: unknown) => {
      // The refusal this page thought it had ruled out: someone moved a task
      // or changed the state list while it was open. Show what the server
      // said and read both lists again, so the row tells the truth next.
      if (caught instanceof ApiError && caught.status === 409) {
        void afterMutation();
      }
    },
  });

  const busy = rename.isPending || move.isPending || remove.isPending;
  const count = counts?.[state.name] ?? 0;

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

  function submitRename(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const next = draft.trim();
    if (next === state.name) {
      setEditing(false);
      return;
    }
    const invalid = stateNameError(next);
    setDraftError(invalid);
    if (invalid !== null) {
      return;
    }
    rename.mutate(next);
  }

  function onRemove() {
    if (
      !window.confirm(
        `Remove ${state.name}? The board loses the column, and agents stop seeing it as a place work can be.`,
      )
    ) {
      return;
    }
    remove.mutate();
  }

  const failure = remove.error ?? move.error;

  return (
    <>
      <tr className="border-console-border/60 border-b last:border-b-0">
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
                className="border-console-border bg-console-bg text-console-text aria-invalid:border-state-failed w-40 rounded border px-2 py-1 font-mono text-xs"
              />
              <SubmitButton loading={rename.isPending}>Save</SubmitButton>
              <SubmitButton
                type="button"
                variant="ghost"
                loading={false}
                disabled={rename.isPending}
                onClick={() => {
                  setEditing(false);
                }}
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
              loading={false}
              disabled={busy || index === 0}
              onClick={() => {
                move.mutate(index - 1);
              }}
            >
              <span aria-hidden="true">↑</span>
              <span className="sr-only">Move {state.name} up</span>
            </SubmitButton>

            <SubmitButton
              type="button"
              variant="ghost"
              loading={false}
              disabled={busy || index === states.length - 1}
              onClick={() => {
                move.mutate(index + 1);
              }}
            >
              <span aria-hidden="true">↓</span>
              <span className="sr-only">Move {state.name} down</span>
            </SubmitButton>

            {!editing && (
              <SubmitButton
                type="button"
                variant="ghost"
                loading={false}
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
                loading={remove.isPending}
                disabled={busy || refusal !== null}
                onClick={onRemove}
              >
                Remove
              </SubmitButton>
            </span>
          </div>
        </td>
      </tr>

      {(draftError !== null || rename.error !== null) && (
        <tr className="border-console-border/60 border-b last:border-b-0">
          <td colSpan={5} className="bg-console-surface/60 px-3 py-2">
            <Alert kind="error">
              {draftError ?? errorMessage(rename.error)}
            </Alert>
          </td>
        </tr>
      )}

      {failure !== null && (
        <tr className="border-console-border/60 border-b last:border-b-0">
          <td colSpan={5} className="bg-console-surface/60 px-3 py-2">
            <Alert kind="error">{errorMessage(failure)}</Alert>
          </td>
        </tr>
      )}
    </>
  );
}

/**
 * The server's own `error` text whenever there is one: `SPEC.md` phrases the
 * refusals — the taken name, the second human state, the four deletion
 * conflicts — better than the client could guess.
 */
function errorMessage(caught: unknown): string {
  if (caught instanceof ApiError) {
    return caught.error;
  }
  console.error(caught);
  return "Something went wrong";
}
