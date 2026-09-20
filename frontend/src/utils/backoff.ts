// The reconnect backoff both real-time streams use: the session WebSocket
// (`session/useSessionSocket.ts`) and the task SSE stream
// (`tasks/useTaskStream.ts`). `SPEC.md`, "Authentication", asks both for the
// same behaviour — one second doubling to a thirty second cap — so the schedule
// lives here once rather than in each hook.

export const BACKOFF_BASE_MS = 1000;
export const BACKOFF_MAX_MS = 30_000;
/** +/- 20%, so two streams dropped by the same restart do not return in step. */
export const BACKOFF_JITTER = 0.2;

/**
 * The delay before retry number `attempt` (zero-based): `1s * 2 ** attempt`,
 * capped at 30 s, spread by `jitter`. Pass `jitter = 0` for an exact schedule.
 */
export function backoffDelay(attempt: number, jitter = BACKOFF_JITTER): number {
  const base = Math.min(BACKOFF_BASE_MS * 2 ** attempt, BACKOFF_MAX_MS);
  return Math.round(base * (1 + (Math.random() * 2 - 1) * jitter));
}
