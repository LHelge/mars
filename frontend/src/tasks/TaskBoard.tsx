// The board of `SPEC.md`, "Frontend", "Task board": one column per task state
// in `position` order, cards in the API's order, and the refresh ordering of
// "Board refresh ordering" made visible.
//
// The board renders the store and nothing else. It never reads the API itself:
// `useTaskStream` (mounted by the view that shows the board) opens the stream
// and owns every refresh, so what is on screen is always a whole REST snapshot
// rather than a snapshot with events applied over it (ADR 0022).
//
// What the board states look like, in the order they are decided:
//   nothing yet          a loading state, and no columns
//   read failed, nothing an error with Retry — an empty board is never shown
//   read failed, loaded  the previous snapshot, with the error above it
//   stale but loaded     the previous snapshot, marked refreshing/reconnecting
//   loaded, no matches   the columns, kept and empty, and `No matching tasks`
//
// Columns are derived with `useMemo` from the arrays the store holds, rather
// than subscribed to through `selectVisibleColumns`: that selector builds
// fresh arrays on every call, so subscribing to it would re-render the board
// on every store write, refresh or not. The search query is one of its inputs,
// so the query is reapplied to whatever the latest snapshot holds without the
// snapshot itself ever being filtered (ADR 0031).

import { useCallback, useMemo, useRef, useState } from "react";

import { Alert } from "../components/Alert";
import { EmptyState } from "../components/EmptyState";
import { LoadingState } from "../components/LoadingState";
import { SectionHeader } from "../components/SectionHeader";
import { SubmitButton } from "../components/SubmitButton";
import type { TaskState } from "../types";
import { taskColumnTestId } from "../utils/testIds";
import { useDelayedFlag } from "../utils/useDelayedFlag";
import { CreateTaskForm } from "./CreateTaskForm";
import { normalizeQuery } from "./search";
import { TaskCard } from "./TaskCard";
import { TaskSearch } from "./TaskSearch";
import { KIND_COLOUR, KIND_MEANING } from "./taskStateRules";
import { selectVisibleColumns, UNKNOWN_COLUMN, useTaskStore } from "./taskStore";
import type { TaskColumn } from "./taskStore";

/** `SPEC.md`, "User-facing features", "Task board", in one line. */
const BOARD_HELP =
  "This project's work, in its own states. Agents claim from a queue state and escalate into the human one; changes they make appear here as they happen.";

/** Why a task can be in a column the project does not have. */
const UNKNOWN_HELP =
  "These tasks are in a state this board does not know. It resolves itself on the next refresh.";

export interface TaskBoardProps {
  projectId: string;
  /**
   * The task of `/projects/:id/tasks/:number`, marked on the board so it can
   * be found behind the drawer that later opens over it.
   */
  openTaskNumber?: number;
}

export function TaskBoard({ projectId, openTaskNumber }: TaskBoardProps) {
  const states = useTaskStore((state) => state.states);
  const tasks = useTaskStore((state) => state.tasks);
  const loaded = useTaskStore((state) => state.loaded);
  const loading = useTaskStore((state) => state.loading);
  const error = useTaskStore((state) => state.error);
  const stream = useTaskStore((state) => state.stream);
  const streamError = useTaskStore((state) => state.streamError);
  const reconnectStream = useTaskStore((state) => state.reconnectStream);
  const refresh = useTaskStore((state) => state.refresh);
  const query = useTaskStore((state) => state.query);
  const setQuery = useTaskStore((state) => state.setQuery);

  const [creating, setCreating] = useState(false);
  const searchInput = useRef<HTMLInputElement>(null);

  // A refresh is two REST reads and is normally over before anyone could read
  // a word about it, while a stream that is not live stays that way: only the
  // first is delayed. Both share one `role="status"`, which is why a marker
  // that flipped on every task event was a screen reader talking over an
  // agent's whole working session (`SPEC.md`, "Frontend", "Board refresh
  // ordering").
  const slowRefresh = useDelayedFlag(loading);

  const columns = useMemo(
    () => selectVisibleColumns({ states, tasks, query }),
    [states, tasks, query],
  );

  const clearSearch = useCallback(() => {
    setQuery("");
    searchInput.current?.focus();
  }, [setQuery]);

  // The no-matches state: a snapshot is on screen and the query matches none
  // of it. It is decided from `loaded` and the query alone, so an empty
  // project with no query keeps its
  // `No tasks yet` invitation and a first load keeps its spinner.
  const noMatches =
    loaded &&
    normalizeQuery(query) !== "" &&
    columns.length > 0 &&
    columns.every((column) => column.tasks.length === 0);

  const newTask = (
    <SubmitButton
      type="button"
      disabled={states.length === 0}
      onClick={() => {
        setCreating(true);
      }}
    >
      New task
    </SubmitButton>
  );

  const retry = (
    <SubmitButton
      type="button"
      variant="ghost"
      loading={loading}
      onClick={() => {
        void refresh();
      }}
    >
      Retry
    </SubmitButton>
  );

  return (
    <section className="space-y-3">
      <SectionHeader
        title="Task board"
        description={BOARD_HELP}
        actions={
          <>
            {loaded && (slowRefresh || stream !== "live") && (
              <span
                role="status"
                className="text-console-muted font-mono text-xs"
              >
                {stream === "reconnecting"
                  ? "Reconnecting"
                  : stream === "offline"
                    ? "Disconnected"
                    : "Refreshing"}
              </span>
            )}
            {!creating && newTask}
          </>
        }
      />

      {/* The stream gave up: the board below is a snapshot that will not
          update itself again until this is answered (`SPEC.md`, "Frontend",
          "Task board"). */}
      {stream === "offline" && streamError !== null && (
        <Alert kind="error">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span>{streamError}</span>
            {reconnectStream !== null && (
              <SubmitButton
                type="button"
                variant="ghost"
                onClick={reconnectStream}
              >
                Reconnect
              </SubmitButton>
            )}
          </div>
        </Alert>
      )}

      {error !== null && (
        <Alert kind="error">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span>{error}</span>
            {retry}
          </div>
        </Alert>
      )}

      {creating && (
        <CreateTaskForm
          projectId={projectId}
          states={states}
          tasks={tasks}
          onClose={() => {
            setCreating(false);
          }}
        />
      )}

      <TaskSearch inputRef={searchInput} onClear={clearSearch} />

      {!loaded ? (
        // Before the first successful load there is no board to show. An
        // error has already been reported above; this is the other half.
        error === null && <LoadingState label="Loading the task board" />
      ) : (
        <>
          {tasks.length === 0 && !noMatches && (
            <EmptyState
              title="No tasks yet"
              description="Create the first one; agents pick their work up from the queue states below."
              action={creating ? undefined : newTask}
            />
          )}

          {columns.length === 0 ? (
            <EmptyState
              title="This project has no task states"
              description="A board needs columns. Add them on the States tab."
            />
          ) : (
            // The columns stay whatever the query hides, so the board keeps
            // its shape while searching and the message below says why they
            // are empty.
            <>
              <div className="flex snap-x gap-3 overflow-x-auto pb-2">
                {columns.map((column) => (
                  <BoardColumn
                    key={column.key}
                    column={column}
                    openTaskNumber={openTaskNumber}
                  />
                ))}
              </div>

              {noMatches && (
                <EmptyState
                  title="No matching tasks"
                  description="Nothing in this project matches that title or number. Clear the search to see the whole board."
                  action={
                    <SubmitButton
                      type="button"
                      variant="ghost"
                      onClick={clearSearch}
                    >
                      Clear search
                    </SubmitButton>
                  }
                />
              )}
            </>
          )}
        </>
      )}
    </section>
  );
}

/**
 * The kind marker: a queue state is work waiting to be claimed, the human
 * state is where escalations land, and a terminal state is spent. Each is one
 * glyph, carrying the kind's colour, with the meaning as its tooltip — a
 * second word in every heading would say the same thing six times over.
 */
const KIND_MARK: Record<TaskState["kind"], string> = {
  queue: "▸",
  human: "!",
  terminal: "■",
};

interface BoardColumnProps {
  column: TaskColumn;
  openTaskNumber: number | undefined;
}

function BoardColumn({ column, openTaskNumber }: BoardColumnProps) {
  const kind = column.state?.kind;

  return (
    <div
      // The end-to-end suite addresses a column by its state's name: the
      // heading alone is ambiguous against the cards' own headings.
      data-testid={taskColumnTestId(column.name)}
      className="flex w-64 shrink-0 snap-start flex-col gap-2"
    >
      <div className="border-console-border flex items-baseline gap-2 border-b pb-1.5">
        <span
          aria-hidden="true"
          title={kind === undefined ? UNKNOWN_HELP : KIND_MEANING[kind]}
          className={`font-mono text-xs ${kind === undefined ? "text-console-muted" : KIND_COLOUR[kind]}`}
        >
          {kind === undefined ? "?" : KIND_MARK[kind]}
        </span>
        <h3
          title={kind === undefined ? UNKNOWN_HELP : KIND_MEANING[kind]}
          className={`min-w-0 flex-1 truncate font-mono text-xs ${column.name === UNKNOWN_COLUMN ? "text-console-muted" : "text-console-text"}`}
        >
          {column.name}
        </h3>
        <span className="text-console-muted font-mono text-xs">
          {column.tasks.length}
        </span>
      </div>

      <div className="flex flex-col gap-2">
        {column.tasks.map((task) => (
          <TaskCard
            key={task.id}
            task={task}
            selected={task.number === openTaskNumber}
          />
        ))}
      </div>
    </div>
  );
}
