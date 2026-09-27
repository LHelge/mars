// The pure part of the task-states editor: the name rule of `SPEC.md`, "Task
// states", and the five deletion refusals the API answers with 409.
//
// The refusals are duplicated here on purpose. `SPEC.md` phrases them as
// conflicts — while any task is in the state, for the `human` state, for the
// last `queue` state, for the last `terminal` state and for a state another
// state names as its `conflict_state` — and every one of them
// is knowable from the list already on screen, so the editor disables the
// button and says why instead of letting the user find out by being refused.
// The server stays the authority: a refusal that still arrives (another user
// moved a task in the meantime) is shown as it came.
//
// It lives beside `TaskStatesEditor.tsx` rather than in it because a module
// that renders a component exports nothing else
// (`react-refresh/only-export-components`).

import type { TableColumn } from "../components/tableStyles";
import type { Task, TaskState, TaskStateKind } from "../types";

/** The editor's table, shared with the row that spans it for its answers. */
export const STATE_COLUMNS: readonly TableColumn[] = [
  { label: "#", className: "hidden w-8 sm:table-cell" },
  { label: "Name" },
  { label: "Kind" },
  { label: "Auto-merge" },
  { label: "Tasks", className: "hidden text-right sm:table-cell" },
  { label: "Actions", className: "pr-0 text-right" },
];

/** Shown instead of a count while the task list has not been read. */
export const COUNTS_UNKNOWN = "Task counts are not loaded";

/** `SPEC.md`: 1–32 characters matching `[a-z0-9][a-z0-9_-]*`. */
export const STATE_NAME_PATTERN = /^[a-z0-9][a-z0-9_-]{0,31}$/;

/** What the add and rename fields say when the name does not match. */
export const NAME_RULE = "Name must be 1–32 characters of a-z, 0-9, _ or -";

/**
 * Why the `human` option is closed once the project has one: `POST` answers
 * 409 for a second human state, and a project has exactly one
 * (`ARCHITECTURE.md`, "Task tracker", "State is a queue").
 */
export const HUMAN_TAKEN = "This project already has a human state";

/** Null when the name is one the API will accept. */
export function stateNameError(name: string): string | null {
  return STATE_NAME_PATTERN.test(name) ? null : NAME_RULE;
}

/**
 * How many tasks sit in each state, keyed by the state's name.
 *
 * A `Map` and not an object, because the key is a state name the user chose.
 * `[a-z0-9][a-z0-9_-]*` admits `constructor`, `toString` and `valueOf`, and a
 * plain object hands those back off `Object.prototype` instead of reporting
 * nothing: the count becomes a function, `count > 0` compares against `NaN` and
 * the editor offers to delete a state that still holds work.
 */
export function countTasksByState(tasks: Task[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const task of tasks) {
    counts.set(task.state, (counts.get(task.state) ?? 0) + 1);
  }
  return counts;
}

/**
 * Why this state cannot be deleted, or null when it can be.
 *
 * The structural reasons come first: a state that is the project's only queue
 * stays undeletable however its tasks move, while a task count is a fact about
 * this minute and changes on its own.
 */
export function deletionReason(
  state: TaskState,
  states: readonly TaskState[],
  taskCounts: ReadonlyMap<string, number>,
): string | null {
  if (state.kind === "human") {
    return "The human state cannot be deleted";
  }
  if (state.kind === "queue" && countKind(states, "queue") === 1) {
    return "The last queue state cannot be deleted";
  }
  if (state.kind === "terminal" && countKind(states, "terminal") === 1) {
    return "The last terminal state cannot be deleted";
  }
  // `SPEC.md`: 409 `state is the conflict state of <name>`, named here so the
  // user knows which row to change first.
  const dependant = states.find(
    (other) => other.id !== state.id && other.conflict_state === state.name,
  );
  if (dependant !== undefined) {
    return `This is the conflict state of ${dependant.name}`;
  }

  const inState = taskCounts.get(state.name) ?? 0;
  if (inState > 0) {
    return inState === 1
      ? "1 task is in this state"
      : `${String(inState)} tasks are in this state`;
  }
  return null;
}

function countKind(states: readonly TaskState[], kind: TaskStateKind): number {
  return states.filter((state) => state.kind === kind).length;
}

/** What each kind means to the orchestrator, in one line, as the tooltip. */
export const KIND_MEANING: Record<TaskStateKind, string> = {
  queue: "Agents claim work from this state",
  human: "Escalations land here; agents never claim from it",
  terminal: "Closes the task and satisfies dependencies",
};

/**
 * Quiet colour reserved for state (`CLAUDE.md`, "Frontend conventions"): the
 * queue an agent picks from carries the accent, the human state the escalation
 * colour the rest of the console uses for it, and a terminal state is spent
 * and reads as quiet as it is.
 */
export const KIND_COLOUR: Record<TaskStateKind, string> = {
  queue: "text-console-accent",
  human: "text-state-human",
  terminal: "text-console-muted",
};

/**
 * The kind marker of a board column: a queue state is work waiting to be
 * claimed, the human state is where escalations land, and a terminal state is
 * spent. One glyph in the kind's colour, named by `KIND_MEANING` and in the
 * board's legend — a second word in every heading would say the same thing
 * six times over.
 */
export const KIND_MARK: Record<TaskStateKind, string> = {
  queue: "▸",
  human: "!",
  terminal: "■",
};
