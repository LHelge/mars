// What the project page says about a failed request. The server's own `error`
// text is shown whenever there is one — `SPEC.md`, "Projects", phrases the
// refusals (a delete while a session is running, a fetch on a project that is
// not `ready`) better than the client could guess.

import { ApiError } from "../../services/apiClient";

const FALLBACK = "Something went wrong";

export function projectErrorMessage(caught: unknown): string {
  if (caught instanceof ApiError) {
    return caught.error;
  }
  console.error(caught);
  return FALLBACK;
}

/** True for the 404 that means the id in the URL names no project. */
export function isNotFound(caught: unknown): boolean {
  return caught instanceof ApiError && caught.status === 404;
}

/**
 * A v4 UUID as the API spells its ids. A path parameter that is not one names
 * no project, so the page answers not-found without asking the server.
 */
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function isUuid(value: string | undefined): value is string {
  return value !== undefined && UUID.test(value);
}
