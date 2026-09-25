// What the Branches tab's history reads out of `HistoryEntry[]` (`SPEC.md`,
// "Git": `GET .../git/history` and `POST .../git/revert`; "Frontend", Project
// page). Pure, so the rows a revert undoes and the tasks it would reopen are
// computed once, from the pages already loaded, and tested beside this file.

import type {
  HistoryEntry,
  HistoryTask,
  TaskState,
  TaskStateKind,
} from "../../types";

/**
 * Entries per page. Smaller than the server's 50: the section sits between the
 * integration heads and the merge form, so a page is what fits in view.
 */
export const HISTORY_PAGE_SIZE = 25;

/**
 * The `before` cursor of the page after `page`: its last commit, or
 * `undefined` when it came back short and so was the end of the line.
 */
export function nextHistoryCursor(
  page: readonly HistoryEntry[],
): string | undefined {
  if (page.length < HISTORY_PAGE_SIZE) {
    return undefined;
  }
  return page.at(-1)?.commit;
}

/** Who asked for a commit, read from its `Requested-By` trailer. */
export type RequestedBy =
  | { kind: "user"; id: string }
  | { kind: "session"; id: string }
  | { kind: "system" }
  /** A spelling this build does not know, shown as written. */
  | { kind: "other"; text: string };

export function parseRequestedBy(value: string | null): RequestedBy | null {
  if (value === null || value === "") {
    return null;
  }
  if (value === "system") {
    return { kind: "system" };
  }
  const colon = value.indexOf(":");
  const prefix = colon < 0 ? "" : value.slice(0, colon);
  const id = value.slice(colon + 1);
  if (id !== "" && (prefix === "user" || prefix === "session")) {
    return { kind: prefix, id };
  }
  return { kind: "other", text: value };
}

/**
 * The entries a revert to `to` undoes: every entry above it, newest first —
 * the first-parent range `to..head` the server will name in `reverted`.
 * `null` when `to` is not loaded or is the head itself, which has nothing to
 * undo (the server refuses `to == head`).
 *
 * The pages are contiguous from the head down, so every entry above a loaded
 * one is loaded too: the list a confirmation shows is always complete.
 */
export function revertRange(
  entries: readonly HistoryEntry[],
  to: string,
): HistoryEntry[] | null {
  const index = entries.findIndex((entry) => entry.commit === to);
  return index <= 0 ? null : entries.slice(0, index);
}

/**
 * The tasks a range names, each once, in the order the range first names them
 * — the order the server's `reopened` follows.
 */
export function rangeTasks(range: readonly HistoryEntry[]): HistoryTask[] {
  const seen = new Set<string>();
  const tasks: HistoryTask[] = [];
  for (const entry of range) {
    for (const task of entry.tasks) {
      if (!seen.has(task.id)) {
        seen.add(task.id);
        tasks.push(task);
      }
    }
  }
  return tasks;
}

/** The kinds a revert may reopen tasks into (400 for a terminal one). */
const REOPEN_KINDS: readonly TaskStateKind[] = ["queue", "human"];

/** The project's states a revert may move its tasks to, in board order. */
export function reopenStates(states: readonly TaskState[]): TaskState[] {
  return states.filter((state) => REOPEN_KINDS.includes(state.kind));
}
