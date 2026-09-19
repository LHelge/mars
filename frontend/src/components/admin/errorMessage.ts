// One reading of a failed administration call. The server's own `error` text is
// shown verbatim — the last-administrator and self-deletion refusals of
// `SPEC.md`, "Users (`/api/users`)", are worded by the backend, so a wording
// change there needs no change here.

import { ApiError } from "../../services";

/** What the user sees when the failure carries no `{ status, error }` body. */
const FALLBACK = "Something went wrong";

export function errorMessage(caught: unknown): string {
  if (caught instanceof ApiError) {
    return caught.error;
  }
  if (caught instanceof TypeError) {
    // `fetch` rejects with a `TypeError` when it never reached the server.
    return "Orchestrator unreachable";
  }
  console.error(caught);
  return FALLBACK;
}
