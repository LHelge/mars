// The pure part of a queue state's auto-merge setting (`SPEC.md`, "Task
// states"; ADR 0045): which states a conflicting merge may send a task to,
// which one turning the toggle on pre-selects, the pair one `PUT` carries, and
// the server's 400s in the editor's words.
//
// `auto_merge` and `conflict_state` travel together on `PUT`: a body that gives
// either replaces both. The editor therefore always sends both, so what is
// stored is exactly the pair on screen and never half of it.
//
// It lives beside `StateRow.tsx` rather than in it because a module that
// renders a component exports nothing else
// (`react-refresh/only-export-components`).

import { errorMessage } from "../services/errorMessage";
import { ApiError } from "../services/apiClient";
import type { TaskState, UpdateTaskStateInput } from "../types";

/** The state `SPEC.md` names as a new project's conflict state. */
export const PREFERRED_CONFLICT_STATE = "ready";

/** Why the toggle is closed on a queue state with nowhere to send a conflict. */
export const NO_CONFLICT_TARGET =
  "Auto-merge needs another queue state to send conflicts to";

/** What an auto-merge state does, in one line, as the chip's tooltip. */
export function autoMergeMeaning(conflictState: string | null): string {
  const target =
    conflictState === null
      ? ""
      : `; a conflict sends the task to ${conflictState}`;
  return `Approved hand-offs here are merged into the default branch automatically${target}`;
}

/**
 * The states a conflicting merge out of `state` may go to: the project's other
 * queue states, in position order — `conflict_state must name a queue state of
 * this project` and `must be a different state`.
 */
export function conflictCandidates(
  state: TaskState,
  states: readonly TaskState[],
): TaskState[] {
  return states
    .filter((other) => other.kind === "queue" && other.id !== state.id)
    .sort((a, b) => a.position - b.position);
}

/**
 * What turning the toggle on pre-selects: `ready` when the project has it as
 * another queue state, else the first other queue state, else null — and a
 * null means the toggle cannot be turned on at all.
 */
export function defaultConflictState(
  state: TaskState,
  states: readonly TaskState[],
): string | null {
  const candidates = conflictCandidates(state, states);
  const preferred = candidates.find(
    (candidate) => candidate.name === PREFERRED_CONFLICT_STATE,
  );
  return (preferred ?? candidates[0])?.name ?? null;
}

/** What the row's toggle and select hold between edits. */
export interface AutoMergeDraft {
  autoMerge: boolean;
  /** Only meaningful while `autoMerge` is on. */
  conflictState: string | null;
}

export function toAutoMergeDraft(state: TaskState): AutoMergeDraft {
  return { autoMerge: state.auto_merge, conflictState: state.conflict_state };
}

/**
 * The draft after the toggle is flipped. Turning it on keeps a conflict state
 * the draft already names and otherwise pre-selects one; turning it off keeps
 * the name, so flipping back restores the user's choice until they save.
 */
export function toggleAutoMerge(
  draft: AutoMergeDraft,
  on: boolean,
  state: TaskState,
  states: readonly TaskState[],
): AutoMergeDraft {
  if (!on) {
    return { ...draft, autoMerge: false };
  }
  const candidates = conflictCandidates(state, states);
  const kept = candidates.some(
    (candidate) => candidate.name === draft.conflictState,
  );
  return {
    autoMerge: true,
    conflictState: kept
      ? draft.conflictState
      : defaultConflictState(state, states),
  };
}

/** True when the draft is not what the state already stores. */
export function autoMergeChanged(
  draft: AutoMergeDraft,
  state: TaskState,
): boolean {
  if (draft.autoMerge !== state.auto_merge) {
    return true;
  }
  return draft.autoMerge && draft.conflictState !== state.conflict_state;
}

/**
 * The `PUT` body: both fields, always. Off is an explicit
 * `conflict_state: null`, which is what `SPEC.md` says `auto_merge: false`
 * leaves behind anyway.
 */
export function toAutoMergeUpdate(draft: AutoMergeDraft): UpdateTaskStateInput {
  return draft.autoMerge
    ? { auto_merge: true, conflict_state: draft.conflictState }
    : { auto_merge: false, conflict_state: null };
}

/**
 * The exact 400s of `SPEC.md`, "Task states", as the row says them. Anything
 * else — a 409, a network failure — is `errorMessage`'s. A `Map`, because the
 * key is the server's text and a plain object answers `constructor`.
 */
const REFUSALS: ReadonlyMap<string, string> = new Map([
  [
    "auto_merge applies to queue states only",
    "Only a queue state can merge automatically.",
  ],
  [
    "auto_merge requires conflict_state",
    "Choose the state a conflicting merge sends the task to.",
  ],
  [
    "conflict_state requires auto_merge",
    "A conflict state is only kept while auto-merge is on.",
  ],
  [
    "conflict_state must name a queue state of this project",
    "The conflict state must be another queue state of this project; the list may have changed since it was loaded.",
  ],
  [
    "conflict_state must be a different state",
    "The conflict state must be a different state.",
  ],
]);

export function autoMergeRefusal(caught: unknown): string {
  if (caught instanceof ApiError && caught.status === 400) {
    const mapped = REFUSALS.get(caught.error);
    if (mapped !== undefined) {
      return mapped;
    }
  }
  return errorMessage(caught);
}
