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

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useMemo } from "react";
import { EmptyState } from "../components/EmptyState";
import { LoadingState } from "../components/LoadingState";
import { QueryErrorAlert } from "../components/QueryErrorAlert";
import { SectionHeader } from "../components/SectionHeader";
import { TableHead } from "../components/TableHead";
import { TABLE, X_SCROLLER } from "../components/tableStyles";
import { errorMessage } from "../services/errorMessage";
import { listTasks } from "../services/tasks";
import { listTaskStates } from "../services/taskStates";
import { AddStateForm } from "./AddStateForm";
import { taskKeys, taskStateKeys } from "./queryKeys";
import { StateRow } from "./StateRow";
import {
  countTasksByState,
  COUNTS_UNKNOWN,
  STATE_COLUMNS,
} from "./taskStateRules";
import { useTaskStore } from "./taskStore";

/** `SPEC.md`, "Task states", in the one line the tab has room for. */
const STATES_HELP =
  "The columns of this project's board, in order. Agents claim work from a queue state, escalations land in the human state, and a terminal state closes the task.";

export interface TaskStatesEditorProps {
  projectId: string;
}

export function TaskStatesEditor({ projectId }: TaskStatesEditorProps) {
  const queryClient = useQueryClient();

  const states = useQuery({
    queryKey: taskStateKeys.list(projectId),
    queryFn: () => listTaskStates(projectId),
  });

  const tasks = useQuery({
    queryKey: taskKeys.all(projectId),
    queryFn: () => listTasks(projectId),
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
        <QueryErrorAlert query={states} message={errorMessage(states.error)} />
      )}

      {tasks.isError && (
        <QueryErrorAlert
          kind="warning"
          query={tasks}
          message={`${COUNTS_UNKNOWN}: ${errorMessage(tasks.error)}. Removing a state waits until they are.`}
        />
      )}

      <AddStateForm
        projectId={projectId}
        hasHuman={hasHuman}
        stateCount={rows.length}
        afterMutation={afterMutation}
      />

      {/* A failed read never says the project has no states (`SPEC.md`,
          "Frontend", Read failures). */}
      {states.isPending ? (
        <LoadingState label="Loading task states" />
      ) : rows.length === 0 ? (
        states.isSuccess && (
          <EmptyState
            title="No task states"
            description="Add the first queue state above; the board has no columns until one exists."
          />
        )
      ) : (
        <div className={X_SCROLLER}>
          <table className={TABLE}>
            <TableHead columns={STATE_COLUMNS} />
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
