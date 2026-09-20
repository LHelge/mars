// The `data-testid` hooks the test suites address the application by.
//
// A `data-testid` is a contract between a component and a test: a rename that
// only touches one side leaves a test failing on a selector instead of on the
// behaviour it was written for. Both sides therefore import the same constants
// from here — the components below, and the Playwright suite through
// `tests/utils/test-ids.ts`, which re-exports this module — so a rename fails
// `tsc -b` instead of a run.
//
// Only hooks the suites really need live here. An element a test can reach by
// role, label or text is reached that way (`CLAUDE.md`, "Testing expectations"):
// these four are the exceptions, where the DOM carries no accessible name
// (a scroll container, a nesting wrapper) or where the name is not unique
// (a board column, a card).
//
// This module is deliberately *not* re-exported from `src/utils/index.ts`: that
// barrel is in the entry chunk, and every consumer here is behind a lazily
// loaded route (`SPEC.md`, "Frontend" → "Code splitting"). Import it by path.

/** The transcript's virtualised scroll container (`src/session/Transcript.tsx`). */
export const TRANSCRIPT_SCROLL = "transcript-scroll";

/** The nested transcript under a subagent tool (`src/session/SubagentGroup.tsx`). */
export const SUBAGENT_CHILDREN = "subagent-children";

/** The caret shown while an assistant message is still streaming. */
export const STREAMING_CURSOR = "streaming-cursor";

/** The prefix every board column's test id starts with. */
export const TASK_COLUMN_PREFIX = "column-";

/** One task-board column, by the state name it shows. */
export function taskColumnTestId(state: string): string {
  return `${TASK_COLUMN_PREFIX}${state}`;
}

/** One card on the board, by its per-project task number. */
export function taskCardTestId(taskNumber: number): string {
  return `task-card-${String(taskNumber)}`;
}
