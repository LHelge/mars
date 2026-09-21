// Which field a refused shared-directory request belongs to.
//
// `SPEC.md`, "Shared directories": a create is 400 for an invalid name or path
// and 409 when the name or the path is already used in the project, and a
// clear or a remove is 409 while any session of the project is `running` or
// `creating`. All four messages are written by the orchestrator and name the
// field or the reason better than the client could guess, so the text itself
// is `errorMessage`'s business (`services/errorMessage.ts`); only the field a
// refusal attaches to is decided here.

import { ApiError } from "../../services/apiClient";

/**
 * The field a 400 or 409 from a create belongs to, or `null` when the message
 * names neither — then the form shows it above the fields instead.
 *
 * Decided on the message because the status alone cannot tell a duplicate name
 * from a duplicate path: both are 409 on the same request. This is the one
 * shared-directory behaviour keyed on the orchestrator's wording, and
 * `SPEC.md`, "Frontend", Failure messages, records the coupling.
 */
export function sharedDirErrorField(
  caught: unknown,
): "name" | "containerPath" | null {
  if (!(caught instanceof ApiError)) {
    return null;
  }
  if (caught.status !== 400 && caught.status !== 409) {
    return null;
  }
  const message = caught.error.toLowerCase();
  if (message.includes("name")) {
    return "name";
  }
  if (message.includes("path")) {
    return "containerPath";
  }
  return null;
}
