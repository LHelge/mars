// Where a session lives in the UI (`SPEC.md`, "Frontend", Routes:
// `/sessions/:id`). Pure, so every launch and link spells it the one way.
//
// Imported by its own path, like everything under `utils/`.

/** The canonical application path of one session. */
export function sessionPath(sessionId: string): string {
  return `/sessions/${sessionId}`;
}
