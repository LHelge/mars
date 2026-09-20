// Resolving what the launch form's task field points at.
//
// `SPEC.md`, "Tasks": `GET /projects/{pid}/tasks/{id}` takes a UUID or a
// per-project number, and a value that addresses no task of the project is a
// 404. `POST /projects/{pid}/sessions` accepts the number too, but the form
// resolves first anyway: it has to show the task's title and its current
// hand-off before the user commits to launching against it.
//
// This is form state, not shared server state, so it is a plain effect rather
// than a TanStack query: it is debounced, it is thrown away when the field
// changes, and a title read three minutes ago must not be shown from a cache
// as though it were current.
//
// Only the outcome of a finished lookup is stored, tagged with the value it
// answers for. Everything else — empty, unreadable, still waiting — is derived
// from the field on each render, so no state is written while typing.

import { useEffect, useState } from "react";
import { ApiError } from "../../services/apiClient";
import { getTask } from "../../services/tasks";
import type { TaskDetail } from "../../types";
import { parseTaskRef } from "../../utils/taskRef";

/** Long enough that typing `#12` is one lookup, short enough to feel immediate. */
const DEBOUNCE_MS = 350;

export type TaskLookup =
  | { status: "empty" }
  /** Typed, but neither a number nor a UUID. */
  | { status: "unreadable" }
  | { status: "loading" }
  | { status: "found"; task: TaskDetail }
  | { status: "missing" }
  | { status: "error"; message: string };

/** What one finished lookup answered, and the value it answered for. */
type Resolved = {
  target: string | number;
  result: Extract<TaskLookup, { status: "found" | "missing" | "error" }>;
};

export function useTaskLookup(
  projectId: string,
  raw: string,
  enabled = true,
): TaskLookup {
  const ref = parseTaskRef(raw);
  const target =
    ref === null ? null : ref.kind === "uuid" ? ref.id : ref.number;

  const [resolved, setResolved] = useState<Resolved | null>(null);

  useEffect(() => {
    if (!enabled || target === null) {
      return;
    }

    // Cleared on the next keystroke, so only the last value is ever fetched
    // and a response that arrives after the field moved on is dropped.
    let live = true;
    const timer = setTimeout(() => {
      getTask(projectId, target)
        .then((task) => {
          if (live) {
            setResolved({ target, result: { status: "found", task } });
          }
        })
        .catch((caught: unknown) => {
          if (!live) {
            return;
          }
          if (caught instanceof ApiError) {
            setResolved({
              target,
              result:
                caught.status === 404
                  ? { status: "missing" }
                  : { status: "error", message: caught.error },
            });
            return;
          }
          console.error(caught);
          setResolved({
            target,
            result: { status: "error", message: "Could not read the task" },
          });
        });
    }, DEBOUNCE_MS);

    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [projectId, target, enabled]);

  if (!enabled || raw.trim() === "") {
    return { status: "empty" };
  }
  if (target === null) {
    return { status: "unreadable" };
  }
  return resolved !== null && resolved.target === target
    ? resolved.result
    : { status: "loading" };
}
