// The task board's store: the authoritative REST snapshot of one project's
// task states and tasks, plus the refresh ordering of `SPEC.md`, "Frontend",
// "Board refresh ordering" (ADR 0022).
//
// A `TaskEvent` is a refresh signal and nothing else: its `task`, `states` and
// `comment` payloads are never written into the snapshot, and no raw event
// array is kept. At most one refresh runs at a time; a response dirtied by a
// later event, or belonging to a view that has moved on, is discarded and
// coalesced into exactly one follow-up refresh. A failed read keeps the
// previous snapshot — an empty board is never installed.

import { create, type UseBoundStore, type StoreApi } from "zustand";

import { queryClient } from "../queryClient";
import { listTasks } from "../services/tasks";
import { errorMessage } from "../services/errorMessage";
import { listTaskStates } from "../services/taskStates";
import type { Task, TaskEvent, TaskState } from "../types";
import { taskKeys, taskStateKeys } from "./queryKeys";
import { filterTasks } from "./search";

/** The two reads a refresh makes, injected so tests can control their timing. */
export interface TaskStoreDeps {
  listTasks: (projectId: string) => Promise<Task[]>;
  listTaskStates: (projectId: string) => Promise<TaskState[]>;
}

/** The stream status shown beside a snapshot that may be stale. */
export type TaskStreamStatus = "connecting" | "live" | "reconnecting";

export interface TaskBoardState {
  /** The project the snapshot belongs to; `null` before the first bind. */
  projectId: string | null;
  /** The project's columns, sorted by `position`. */
  states: TaskState[];
  /** The project's tasks in the API's order (priority, then number). */
  tasks: Task[];
  /** Highest `seq` seen on the task stream; the stream's `?after=`. */
  lastSeq: number;
  /** A first successful load has installed a snapshot. */
  loaded: boolean;
  /** A refresh is in flight. */
  loading: boolean;
  /** The snapshot is known to be behind; a refresh is owed. */
  dirty: boolean;
  /** The last read failure, cleared by the next successful load. */
  error: string | null;
  stream: TaskStreamStatus;
  /** Bumped by a project change, a reconnect, an unmount and `reset()`. */
  viewGeneration: number;
  /** Bumped by every non-duplicate event and every local mutation. */
  eventGeneration: number;
  /** The board's search query, kept across drawer route changes. */
  query: string;
}

export interface TaskBoardActions {
  /** Binds the store to a project; a different one resets the snapshot. */
  bindProject: (projectId: string) => void;
  setStream: (status: TaskStreamStatus) => void;
  /**
   * Records a stream event. Returns `false` for a duplicate or replayed
   * `seq` (nothing changes); otherwise advances the cursor, dirties the
   * snapshot, schedules a refresh and returns `true`.
   */
  noteEvent: (event: TaskEvent) => boolean;
  /** Called after every successful local mutation. */
  invalidate: () => void;
  /** Reconnect or unmount: older responses stop being eligible. */
  bumpView: () => void;
  refresh: () => Promise<void>;
  setQuery: (query: string) => void;
  reset: () => void;
}

export type TaskStore = TaskBoardState & TaskBoardActions;

export type TaskStoreHook = UseBoundStore<StoreApi<TaskStore>>;

export function emptyTaskBoardState(): TaskBoardState {
  return {
    projectId: null,
    states: [],
    tasks: [],
    lastSeq: 0,
    loaded: false,
    loading: false,
    dirty: false,
    error: null,
    stream: "connecting",
    viewGeneration: 0,
    eventGeneration: 0,
    query: "",
  };
}

function byPosition(states: TaskState[]): TaskState[] {
  return [...states].sort((a, b) => a.position - b.position);
}

function messageOf(error: unknown): string {
  return errorMessage(error);
}

export function createTaskStore(deps: TaskStoreDeps): TaskStoreHook {
  // A refresh asked for while one was already in flight. The in-flight run
  // consumes it when it settles, so a request is never silently dropped —
  // including the load a freshly bound view asks for while the previous
  // view's reads are still outstanding.
  let pending = false;
  // Identifies the run that owns `loading`. `reset()` clears `loading` under
  // an in-flight run, so a settling run only writes state when it is still
  // the owner.
  let runCounter = 0;
  let activeRun = 0;

  return create<TaskStore>()((set, get) => ({
    ...emptyTaskBoardState(),

    bindProject: (projectId) => {
      if (get().projectId === projectId) return;
      set((state) => ({
        projectId,
        states: [],
        tasks: [],
        query: "",
        lastSeq: 0,
        loaded: false,
        dirty: false,
        error: null,
        stream: "connecting",
        viewGeneration: state.viewGeneration + 1,
      }));
    },

    setStream: (status) => set({ stream: status }),

    noteEvent: (event) => {
      if (event.seq <= get().lastSeq) return false;
      set((state) => ({
        lastSeq: event.seq,
        eventGeneration: state.eventGeneration + 1,
        dirty: true,
      }));
      void get().refresh();
      return true;
    },

    invalidate: () => {
      set((state) => ({
        eventGeneration: state.eventGeneration + 1,
        dirty: true,
      }));
      void get().refresh();
    },

    bumpView: () =>
      set((state) => ({ viewGeneration: state.viewGeneration + 1 })),

    setQuery: (query) => set({ query }),

    refresh: async () => {
      const projectId = get().projectId;
      if (projectId === null) return;
      if (get().loading) {
        // The in-flight run follows up; a second concurrent read would be the
        // very overlap this ordering exists to prevent.
        pending = true;
        return;
      }

      const run = ++runCounter;
      activeRun = run;
      pending = false;
      set({ loading: true, dirty: false });
      const view = get().viewGeneration;
      const events = get().eventGeneration;

      const owns = () => activeRun === run;
      const followUp = () => {
        if (pending || get().dirty) void get().refresh();
      };

      let states: TaskState[];
      let tasks: Task[];
      try {
        [states, tasks] = await Promise.all([
          deps.listTaskStates(projectId),
          deps.listTasks(projectId),
        ]);
      } catch (error) {
        if (!owns()) return;
        if (get().viewGeneration !== view) {
          // The failure belongs to a view nobody is looking at any more.
          set({ loading: false });
          followUp();
          return;
        }
        // The snapshot, `loaded` and `lastSeq` all stand; the next event or
        // the Retry action tries again.
        set({ loading: false, dirty: true, error: messageOf(error) });
        return;
      }

      if (!owns()) return;

      if (get().viewGeneration !== view) {
        // Another project, a reconnect or an unmount: these responses can no
        // longer update the current view, and the new view loads itself.
        set({ loading: false });
        followUp();
        return;
      }

      if (get().eventGeneration !== events) {
        // Dirtied while in flight: discard and coalesce into one follow-up.
        set({ loading: false });
        void get().refresh();
        return;
      }

      set({
        states: byPosition(states),
        tasks,
        loaded: true,
        error: null,
        loading: false,
      });
      followUp();
    },

    reset: () => {
      pending = false;
      set((state) => ({
        ...emptyTaskBoardState(),
        viewGeneration: state.viewGeneration + 1,
      }));
    },
  }));
}

/**
 * The reads of the app singleton go through TanStack Query, so its retry and
 * backoff apply before a failure reaches the board, and the keys the states
 * editor and the task drawer hold are freshened by the same request.
 */
const appDeps: TaskStoreDeps = {
  listTasks: (projectId) =>
    queryClient.fetchQuery({
      queryKey: taskKeys.all(projectId),
      queryFn: () => listTasks(projectId),
      staleTime: 0,
    }),
  listTaskStates: (projectId) =>
    queryClient.fetchQuery({
      queryKey: taskStateKeys.list(projectId),
      queryFn: () => listTaskStates(projectId),
      staleTime: 0,
    }),
};

export const useTaskStore = createTaskStore(appDeps);

/** The heading of the trailing bucket, and its React key. */
export const UNKNOWN_COLUMN = "unknown";

/**
 * One board column: the tasks sitting in it, and the state they are in.
 *
 * `state` is `null` for the trailing bucket that catches tasks whose `state`
 * name is not among the project's current states. That only happens between
 * two reads of one refresh — a state renamed while the board was open — so the
 * bucket is a column without a state row rather than a fabricated `TaskState`:
 * nothing downstream can mistake it for a column the project actually has.
 */
export interface TaskColumn {
  /** Stable across refreshes: the state's id, or `UNKNOWN_COLUMN`. */
  key: string;
  /** What the column is headed with. */
  name: string;
  state: TaskState | null;
  tasks: Task[];
}

/**
 * Columns in `position` order, each keeping the API's task order, followed by
 * the `unknown` bucket when — and only when — some task is in no current
 * state. A task is never dropped; the next consistent snapshot files it.
 *
 * Takes the two arrays rather than the whole state so a view can select them
 * one by one and derive the columns in a `useMemo`: this builds fresh arrays
 * on every call, and a store subscription on it would re-render on each.
 */
export function selectColumns(
  state: Pick<TaskBoardState, "states" | "tasks">,
): TaskColumn[] {
  const known = new Set(state.states.map((column) => column.name));
  const columns: TaskColumn[] = state.states.map((column) => ({
    key: column.id,
    name: column.name,
    state: column,
    tasks: state.tasks.filter((task) => task.state === column.name),
  }));

  const orphans = state.tasks.filter((task) => !known.has(task.state));
  if (orphans.length > 0) {
    columns.push({
      key: UNKNOWN_COLUMN,
      name: UNKNOWN_COLUMN,
      state: null,
      tasks: orphans,
    });
  }
  return columns;
}

/**
 * The columns with the board's search query applied to each, for `SPEC.md`,
 * "Frontend", "Task-board search" (ADR 0031).
 *
 * Filtering happens here and nowhere else: `states` and `tasks` are left as the
 * last snapshot installed them, so clearing the query restores every card
 * without a read, and the next refresh reapplies the query to the titles it
 * brings. Column order and the API's task order inside each column are the
 * ones `selectColumns` produced.
 *
 * Every column the project has stays, empty or not — the query narrows what is
 * on the board, it does not reshape the board. The trailing `unknown` bucket
 * keeps the same rule: it appears when the *unfiltered* snapshot has an orphan
 * and then stays even if the query matches none of them, because a bucket that
 * came and went with the query would look like a state appearing mid-search.
 *
 * Like `selectColumns`, this builds fresh arrays on every call, so a view
 * selects `states`, `tasks` and `query` one by one and derives the columns in a
 * `useMemo` rather than subscribing to this.
 */
export function selectVisibleColumns(
  state: Pick<TaskBoardState, "states" | "tasks" | "query">,
): TaskColumn[] {
  return selectColumns(state).map((column) => ({
    ...column,
    tasks: filterTasks(column.tasks, state.query),
  }));
}

/** The task carrying a per-project `number` (the drawer's route parameter). */
export function selectTaskByNumber(
  number: number,
): (state: TaskBoardState) => Task | undefined {
  return (state) => state.tasks.find((task) => task.number === number);
}

/** The task with this UUID: parent badges and dependency links resolve by id. */
export function selectTaskById(
  id: string,
): (state: TaskBoardState) => Task | undefined {
  return (state) => state.tasks.find((task) => task.id === id);
}
