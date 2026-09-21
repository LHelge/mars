// What the drawer's edit form sends, and what it is allowed to offer
// (`SPEC.md`, "Tasks": the `PUT` body and the one-level parent rules).
//
// Everything here is pure and holds the decisions that are easy to get wrong:
// which fields a save actually carries, whether the server has moved away from
// the reading the form opened on, and which tasks may be named as a parent.
// They live apart from the form because a module that renders a component
// exports nothing else (`react-refresh/only-export-components`).

import type { Task, TaskPriority, UpdateTaskInput } from "../types";

/**
 * The editable half of a task, as the form holds it while it is being typed.
 *
 * `description` is a string and never `null`: a textarea has no third state,
 * and a task with no description opens the form empty.
 */
export interface TaskEditValues {
  title: string;
  description: string;
  priority: TaskPriority;
  labels: string[];
  /** `null` is "unassigned". */
  assignee_user_id: string | null;
  /** `null` is "top-level". */
  parent_id: string | null;
}

/** The form's starting point: the task exactly as the API last answered it. */
export function taskEditValues(task: Task): TaskEditValues {
  return {
    title: task.title,
    description: task.description ?? "",
    priority: task.priority,
    labels: task.labels,
    assignee_user_id: task.assignee_user_id,
    parent_id: task.parent_id,
  };
}

/** Set comparison: the API stores labels as a set, so order is not a change. */
function sameLabels(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false;
  const seen = new Set(a);
  return b.every((label) => seen.has(label));
}

/**
 * Only what changed.
 *
 * A `PUT` leaves every omitted field alone, so sending the untouched ones back
 * would turn a title edit into a write of six columns — and would overwrite,
 * with the values the form opened on, whatever an agent changed meanwhile.
 * Clearing the assignee or the parent is a change to `null`, which is the one
 * value that has to be sent explicitly to mean anything.
 *
 * The title is trimmed: trailing whitespace is a slip, not an edit.
 */
export function diffTaskInput(
  original: TaskEditValues,
  edited: TaskEditValues,
): UpdateTaskInput {
  const input: UpdateTaskInput = {};

  const title = edited.title.trim();
  if (title !== original.title) input.title = title;
  if (edited.description !== original.description) {
    input.description = edited.description;
  }
  if (edited.priority !== original.priority) input.priority = edited.priority;
  if (!sameLabels(original.labels, edited.labels)) input.labels = edited.labels;
  if (edited.assignee_user_id !== original.assignee_user_id) {
    input.assignee_user_id = edited.assignee_user_id;
  }
  if (edited.parent_id !== original.parent_id)
    input.parent_id = edited.parent_id;

  return input;
}

/** Nothing to send: the form closes without a request. */
export function isEmptyUpdate(input: UpdateTaskInput): boolean {
  return Object.keys(input).length === 0;
}

/**
 * True when the two sets of values differ in any editable field.
 *
 * The form opens on one reading of the task and keeps it as its baseline for
 * as long as it is open, so a refresh cannot silently widen a save. It can,
 * though, mean the form is editing values someone else has already moved on
 * from, which is what this answers: the drawer says so instead of staying
 * silent.
 */
export function taskEditValuesDiffer(
  a: TaskEditValues,
  b: TaskEditValues,
): boolean {
  return !isEmptyUpdate(diffTaskInput(a, b));
}

/**
 * The tasks the parent select may offer for `task`.
 *
 * `SPEC.md`, "Tasks": nesting is one level deep, so a parent is a task that
 * has no parent of its own, in this project, and is not the task being
 * edited. A task that already has children is a perfectly good parent — it is
 * the *edited* task having children that forbids the move, and the form
 * disables the select outright in that case rather than offering a list that
 * would all answer 400.
 *
 * The candidates come from the board snapshot, so the form can only propose
 * what the board can see; when the snapshot is a moment behind, the API
 * answers 400 or 404 and the form shows what it said.
 */
export function parentCandidates(tasks: Task[], task: Task): Task[] {
  return tasks.filter(
    (candidate) => candidate.id !== task.id && candidate.parent_id === null,
  );
}
