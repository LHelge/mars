// The task SSE boundary: the one place a `data:` line from
// `/api/projects/{pid}/tasks/stream` becomes a `TaskEvent` the board store may
// be handed (`SPEC.md`, "SSE: task stream", "TaskEvent").
//
// The board never applies an event's payload — it deduplicates on `seq` and
// refreshes over REST (ADR 0022) — so the only field that can do damage is
// `seq` itself, and it does: a non-numeric one makes `event.seq <= lastSeq`
// false, writes itself into `lastSeq`, and the next `?after=` is a cursor the
// server answers 400 BAD_CURSOR to, on every reconnect, for as long as the tab
// is open. So the check is `seq` above all, and the rest of the envelope only
// far enough to know the frame is one of ours.
//
// An unknown `kind` is accepted deliberately. The board's reaction to every
// kind is the same coalesced refresh, so a kind a newer orchestrator added is
// already handled correctly by being counted and refreshed on; rejecting it
// would drop its `seq` and leave the cursor behind.

import type { TaskEvent } from "../types";
import { isRecord, isSeq, isString, parseJson } from "../utils/wire";

/**
 * One `TaskEvent`, or `null` when the frame is not one. The diagnostic the
 * caller writes names the field, never its value (`CLAUDE.md` rule 3).
 */
export function validateTaskEvent(value: unknown): TaskEvent | null {
  if (!isRecord(value)) return null;
  if (!isSeq(value.seq)) return null;
  if (!isString(value.ts)) return null;
  if (!isString(value.kind)) return null;
  // Retained after deletion, and `null` on `states_changed`.
  if (value.task_id !== null && !isString(value.task_id)) return null;
  if (!isRecord(value.actor) || !isString(value.actor.kind)) return null;
  return value as unknown as TaskEvent;
}

export function parseTaskEvent(data: string): TaskEvent | null {
  const value = parseJson(data);
  if (value === undefined) return null;
  return validateTaskEvent(value);
}
