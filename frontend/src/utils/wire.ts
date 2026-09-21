// The small guards the two stream boundaries — the session WebSocket and the
// task SSE stream — check their frames with before anything reaches a store.
//
// `JSON.parse(data) as T` is a compile-time assertion and no more: a frame the
// orchestrator never sent, or one a proxy mangled, arrives typed and wrong.
// These are deliberately primitive. The rule the boundaries follow is in
// `SPEC.md`, "WebSocket: session stream" and "SSE: task stream": check the
// discriminator and the fields the reducer actually reads, nothing else. The
// business rules stay where they are enforced, on the server.
//
// Nothing here ever formats a value into its answer: a rejection says which
// field failed, never what was in it, so a frame carrying agent output or a
// token cannot reach a log through a diagnostic (`CLAUDE.md` rule 3).

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function isString(value: unknown): value is string {
  return typeof value === "string";
}

export function isBoolean(value: unknown): value is boolean {
  return typeof value === "boolean";
}

/** A finite number; `NaN` and the infinities are not values any field takes. */
export function isNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

/**
 * A stream cursor: per-session or per-project, monotonic, starting at 1
 * (`SPEC.md`, "AgentEvent", "TaskEvent"). `Number.isSafeInteger` is the part
 * that matters — a `seq` that is `undefined`, a string or a float is what
 * turns a cursor into `after=undefined` and a reconnect into a loop.
 */
export function isSeq(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 1;
}

export function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every(isString);
}

/** An absent optional field passes; a present one is checked. */
export function isOptional<T>(
  value: unknown,
  guard: (value: unknown) => value is T,
): value is T | undefined {
  return value === undefined || guard(value);
}

/** Parses JSON without throwing; `undefined` means the text was not JSON. */
export function parseJson(text: string): unknown {
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return undefined;
  }
}
