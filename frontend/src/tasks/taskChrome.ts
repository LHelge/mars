// The small visual vocabulary the board and the drawer share, so a task looks
// the same in the column it sits in and in the panel that opens over it.
//
// Colour is information, not decoration (`CLAUDE.md`, "Frontend conventions"):
// only a critical or high priority is set in a state colour, because
// "ordinary" is the default and should not compete for the eye.

import { TAP_INLINE } from "../components/fieldStyles";
import { TASK_PRIORITIES } from "../types";
import type { TaskPriority } from "../types";

/** Quiet colour for the two priorities that mean "not later" (`SPEC.md`). */
export const PRIORITY_COLOUR: Record<TaskPriority, string> = {
  0: "text-state-failed",
  1: "text-state-parked",
  2: "text-console-muted",
  3: "text-console-muted",
};

/** What `P0`..`P3` mean; the board writes the number only. */
export const PRIORITY_MEANING: Record<TaskPriority, string> = {
  0: "P0 — critical",
  1: "P1 — high",
  2: "P2 — normal",
  3: "P3 — low",
};

/**
 * The four priorities as a select's options, in the order they are read —
 * which is the order the union is derived from, so an option and a valid
 * priority are the same list (`parseTaskPriority` reads the same one back).
 */
export const PRIORITIES: readonly TaskPriority[] = TASK_PRIORITIES;

/**
 * What each of a card's chips means, as its accessible name (`TaskCard`,
 * `role="img"` over the short text) and as the board's legend reads it
 * (`BoardLegend`). A chip's text is terse — `↑2`, `P1`, `round 2/5` — and a
 * tooltip is no place for the rest (`SPEC.md`, "Frontend", "Mobile layout").
 */
export const chipMeaning = {
  priority: (priority: TaskPriority): string =>
    `Priority ${PRIORITY_MEANING[priority]}`,
  child: (parentNumber: number | undefined): string =>
    parentNumber === undefined
      ? "Child of another task"
      : `Child of task #${String(parentNumber)}`,
  blocked: "Blocked: waiting on open children or unsatisfied dependencies",
  dependencies: (blockedBy: number, blocking: number): string => {
    const parts: string[] = [];
    if (blockedBy > 0) parts.push(`Blocked by ${count(blockedBy, "task")}`);
    if (blocking > 0) parts.push(`blocks ${count(blocking, "task")}`);
    const sentence = parts.join("; ");
    return sentence.charAt(0).toUpperCase() + sentence.slice(1);
  },
  attempts: (attempts: number): string =>
    `${count(attempts, "session")} picked this task up`,
  round: (rounds: number, maxRounds: number | undefined): string =>
    maxRounds === undefined
      ? `Revision round ${String(rounds)}`
      : `Revision round ${String(rounds)} of ${String(maxRounds)}`,
  assignee: (username: string): string => `Assigned to ${username}`,
  label: (label: string): string => `Label ${label}`,
} as const;

function count(n: number, noun: string): string {
  return `${String(n)} ${noun}${n === 1 ? "" : "s"}`;
}

/** One piece of task metadata: a bordered, monospace micro-tag. */
export const CHIP =
  "border-console-border inline-flex items-center rounded border px-1 py-px font-mono text-[0.6875rem] leading-4";

/**
 * A chip-sized button — `View diff`, `Merge` — for a row of mono metadata that
 * a form-sized button would break. Its hit area on a coarse pointer is
 * `TAP_INLINE`'s, so the row keeps its line height on a phone too (`SPEC.md`,
 * "Frontend", "Mobile layout").
 */
export const CHIP_BUTTON = `border-console-border hover:bg-console-raised hover:text-console-text rounded border px-1.5 py-px font-mono text-[0.6875rem] ${TAP_INLINE}`;

/**
 * The card's round marker (`SPEC.md`, "Frontend", "Task board"): nothing until
 * a second round, then `round 2/5` against the project's `max_rounds`, or
 * `round 2` while the project has not been read.
 */
export function roundLabel(
  rounds: number,
  maxRounds: number | undefined,
): string | null {
  if (rounds <= 1) {
    return null;
  }
  return maxRounds === undefined
    ? `round ${String(rounds)}`
    : `round ${String(rounds)}/${String(maxRounds)}`;
}
