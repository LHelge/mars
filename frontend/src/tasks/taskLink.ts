// Where a task lives in the UI (`SPEC.md`, "Frontend", Routes:
// `/projects/:id/tasks/:number` is the board with that task's drawer open, and
// "Copy links": the absolute form of that same route on the current origin).
//
// Pure, and free of `window`, so the board, the drawer and their tests all
// spell the link the one way.

/** The canonical application path of one task. */
export function taskPath(projectId: string, number: number): string {
  return `/projects/${projectId}/tasks/${String(number)}`;
}

/**
 * The board a task drawer opened over, keeping whatever the board was showing
 * — the search field among it — rather than resetting the view.
 *
 * The drawer's `close` and the action bar's `backToBoard` are the same
 * navigation and spell it here; they are not the same function, because
 * closing the drawer also restores focus to where it came from.
 */
export function boardPath(projectId: string, search: URLSearchParams): string {
  const params = new URLSearchParams(search);
  params.set("tab", "board");
  return `/projects/${projectId}?${params.toString()}`;
}

/**
 * The `:number` of the route as a task number. Task numbers start at 1 and are
 * plain integers, so anything else — a name, `#12`, `0`, a decimal — names no
 * task and is answered without asking the server.
 */
export function parseTaskNumber(raw: string): number | null {
  if (!/^\d+$/.test(raw)) {
    return null;
  }
  const number = Number(raw);
  return Number.isSafeInteger(number) && number > 0 ? number : null;
}
