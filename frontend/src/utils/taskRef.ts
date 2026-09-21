// How the launch form reads what a user typed into its task field.
//
// `SPEC.md`, "Tasks": `GET /projects/{pid}/tasks/{id}` takes a task's UUID or
// its per-project number, and the board writes task numbers as `#12`. The
// session create body, though, wants the UUID, so the form resolves whatever
// was typed through `getTask` before it submits — this parser only decides
// what to send to that lookup, and refuses anything that is neither.

import { isUuid } from "./uuid";

export type TaskRef =
  { kind: "number"; number: number } | { kind: "uuid"; id: string };

const NUMBER = /^#?(\d+)$/;

/**
 * `#12`, `12` or a UUID, with surrounding whitespace ignored. Anything else —
 * including `#0`, a negative number and an empty field — is `null`.
 */
export function parseTaskRef(raw: string): TaskRef | null {
  const value = raw.trim();
  if (value === "") {
    return null;
  }

  if (isUuid(value)) {
    return { kind: "uuid", id: value.toLowerCase() };
  }

  const digits = NUMBER.exec(value);
  if (digits !== null) {
    const number = Number(digits[1]);
    // Task numbers start at 1; `#0` addresses nothing.
    return number > 0 ? { kind: "number", number } : null;
  }

  return null;
}
