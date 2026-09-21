// What a session's `launch_source` is called on screen (`SPEC.md`, "Sessions";
// `ARCHITECTURE.md`, "Task tracker" → "Unattended launches").
//
// The union is handled exhaustively here, in one place, so the schedule label
// is already written the day scheduled agents land and nothing has to be found
// again. A session launched by a person gets no label: `user` is what every
// session in the list already is, and a tag on all of them would say nothing.
//
// It is deliberately not derived from `created_by`. Deleting a user leaves the
// same NULL on the sessions that user launched, so a null there means the
// person is gone, never that nobody launched it.
//
// A module of its own rather than part of the tag component, because a module
// that renders a component exports nothing else
// (`react-refresh/only-export-components`).

import type { LaunchSource } from "../types";

/** The tag's text, or `null` for a session a person launched. */
export function launchSourceLabel(source: LaunchSource): string | null {
  switch (source) {
    case "user":
      return null;
    case "dispatcher":
      return "dispatcher";
    case "schedule":
      return "schedule";
  }
}

/** The full sentence behind the tag, as its tooltip and its accessible name. */
export function launchSourceTitle(source: LaunchSource): string | null {
  switch (source) {
    case "user":
      return null;
    case "dispatcher":
      return "Launched by the dispatcher, not by a person";
    case "schedule":
      return "Launched on a schedule, not by a person";
  }
}
