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
 * The shareable link: an origin and the canonical path, and nothing else — no
 * search parameters, no fragment, no token. A trailing slash on the origin is
 * dropped so the result never carries a doubled separator.
 */
export function buildTaskLink(
  origin: string,
  projectId: string,
  number: number,
): string {
  return `${origin.replace(/\/+$/, "")}${taskPath(projectId, number)}`;
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
