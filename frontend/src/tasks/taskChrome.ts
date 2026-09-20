// The small visual vocabulary the board and the drawer share, so a task looks
// the same in the column it sits in and in the panel that opens over it.
//
// Colour is information, not decoration (`CLAUDE.md`, "Frontend conventions"):
// only a critical or high priority is set in a state colour, because
// "ordinary" is the default and should not compete for the eye.

import type { TaskPriority } from "../types";

/** Quiet colour for the two priorities that mean "not later" (`SPEC.md`). */
export const PRIORITY_COLOUR: Record<TaskPriority, string> = {
  0: "text-state-failed",
  1: "text-state-parked",
  2: "text-console-muted",
  3: "text-console-muted",
};

/** What `P0`..`P3` mean, as a tooltip; the board writes the number only. */
export const PRIORITY_MEANING: Record<TaskPriority, string> = {
  0: "P0 — critical",
  1: "P1 — high",
  2: "P2 — normal",
  3: "P3 — low",
};

/** The four priorities as a select's options, in the order they are read. */
export const PRIORITIES: TaskPriority[] = [0, 1, 2, 3];

/** One piece of task metadata: a bordered, monospace micro-tag. */
export const CHIP =
  "border-console-border inline-flex items-center rounded border px-1 py-px font-mono text-[0.6875rem] leading-4";

/** Every input, select and textarea a task form shows, set like the console. */
export const CONTROL =
  "border-console-border bg-console-bg text-console-text rounded border px-2.5 py-1.5 font-mono text-sm disabled:opacity-50";
