// The task-board search of `SPEC.md`, "Frontend", "Task-board search"
// (ADR 0031): a pure filter over the snapshot the board already holds.
//
// Nothing here reads the API or the store. The board derives its visible cards
// by running `filterTasks` over the complete snapshot on every render, so a
// refresh that renames, adds or removes a task reapplies the query for free.
//
// Only `title` and `number` are looked at. Descriptions and comments are
// deliberately out of scope: they are not loaded with the board snapshot, and
// searching them would need the API contract this ADR exists to avoid.

import type { Task } from "../types";

/** A query that is only digits, with an optional `#`, means "this number". */
const NUMBER_QUERY = /^#?\d+$/;

/**
 * The query as it is matched: surrounding whitespace is not part of what the
 * user meant. Interior spaces are, so ` fix login ` still searches for the
 * two-word phrase.
 */
export function normalizeQuery(raw: string): string {
  return raw.trim();
}

/**
 * The tasks matching `query`, in the order they came in.
 *
 * An empty (or whitespace-only) query returns the very array it was given, so
 * an unfiltered board hands its columns the same references every render.
 *
 * A digits-only query, with or without a leading `#`, matches one exact task
 * number: `42` finds task 42 and neither 142 nor 420. The comparison is made
 * between strings, after dropping leading zeros, so `042` still finds task 42
 * and a digit string too long for a float — where `Number` would round several
 * distinct inputs onto the same value — matches nothing rather than the wrong
 * task. Anything else, `#` alone included, is a case-insensitive substring of
 * the title.
 */
export function filterTasks(tasks: Task[], query: string): Task[] {
  const normalized = normalizeQuery(query);
  if (normalized === "") return tasks;

  if (NUMBER_QUERY.test(normalized)) {
    const digits = normalized.replace(/^#/, "").replace(/^0+(?=\d)/, "");
    return tasks.filter((task) => String(task.number) === digits);
  }

  const needle = normalized.toLowerCase();
  return tasks.filter((task) => task.title.toLowerCase().includes(needle));
}
