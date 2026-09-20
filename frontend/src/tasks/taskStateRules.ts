// The pure part of the task-states editor: the name rule of `SPEC.md`, "Task
// states", and the four deletion refusals the API answers with 409.
//
// The refusals are duplicated here on purpose. `SPEC.md` phrases them as
// conflicts — while any task is in the state, for the `human` state, for the
// last `queue` state and for the last `terminal` state — and every one of them
// is knowable from the list already on screen, so the editor disables the
// button and says why instead of letting the user find out by being refused.
// The server stays the authority: a refusal that still arrives (another user
// moved a task in the meantime) is shown as it came.
//
// It lives beside `TaskStatesEditor.tsx` rather than in it because a module
// that renders a component exports nothing else
// (`react-refresh/only-export-components`).

import type { Task, TaskState, TaskStateKind } from "../types";

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

/** How many tasks sit in each state, keyed by the state's name. */
export function countTasksByState(tasks: Task[]): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const task of tasks) {
    counts[task.state] = (counts[task.state] ?? 0) + 1;
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
  states: TaskState[],
  taskCounts: Record<string, number>,
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

  const inState = taskCounts[state.name] ?? 0;
  if (inState > 0) {
    return inState === 1
      ? "1 task is in this state"
      : `${String(inState)} tasks are in this state`;
  }
  return null;
}

function countKind(states: TaskState[], kind: TaskStateKind): number {
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
