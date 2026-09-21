// One reading of a failed request for the whole frontend (`SPEC.md`,
// "Frontend", Failure messages). Every feature that turns a caught value into
// a sentence goes through `errorMessage`; a per-feature helper only adds its
// own special cases on top of it and delegates the rest.
//
// This module and `./apiClient` import each other — the client asks for the
// status wording, this module reads `ApiError` — and, like the client's cycle
// with `./auth`, neither calls across it at module load.

import { ApiError } from "./apiClient";

/** A request that never reached the orchestrator, or one it never answered. */
export const UNREACHABLE = "Orchestrator unreachable";

/** A failure that is neither the server's to explain nor ours to guess. */
export const GENERIC_FAILURE = "Something went wrong";

/**
 * The statuses a reverse proxy writes when the orchestrator is not there to
 * answer. nginx answers them with its own HTML, so they carry no
 * `{ status, error }` body to quote.
 */
const NO_ANSWER = new Set([502, 503, 504]);

/**
 * What the user reads for a response with no `{ status, error }` body. Never
 * `Response.statusText`: it is always empty under HTTP/2 and HTTP/3, which is
 * how the browser talks to nginx in any TLS deployment.
 */
export function statusMessage(status: number): string {
  return NO_ANSWER.has(status) ? UNREACHABLE : `HTTP ${status}`;
}

/**
 * A failure whose message is already the sentence to show. For the cases the
 * client itself decides — an invariant it checks before calling anything, a
 * reply that is well-formed but useless — where forging an `ApiError` with a
 * status the server never sent would be a lie.
 */
export class MessageError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "MessageError";
  }
}

/** `fetch` rejects with a `TypeError` when the request never reached a server. */
export function isNetworkFailure(caught: unknown): boolean {
  return caught instanceof TypeError;
}

/**
 * The one error-to-text rule:
 *
 * - an `ApiError` carrying words is shown in the server's own words, whatever
 *   the status — `SPEC.md` phrases its refusals, and its 5xx bodies are
 *   generic by `CLAUDE.md` rule 3, so there is nothing to hide and nothing to
 *   improve on;
 * - an `ApiError` carrying none — a proxy's HTML, an empty body — becomes the
 *   status wording, so an empty string never reaches an `Alert`;
 * - a network failure says the orchestrator could not be reached;
 * - anything else is a bug here, and reads as `fallback`.
 *
 * Pure: it is safe to call during render. The failure is logged where it is
 * caught, with `logUnexpected`.
 */
export function errorMessage(
  caught: unknown,
  fallback: string = GENERIC_FAILURE,
): string {
  if (caught instanceof ApiError) {
    const words = caught.error.trim();
    if (words !== "") {
      return caught.error;
    }
    // Status 0 is not a status: nothing built this from a response.
    return caught.status > 0 ? statusMessage(caught.status) : fallback;
  }
  if (caught instanceof MessageError) {
    return caught.message;
  }
  if (isNetworkFailure(caught)) {
    return UNREACHABLE;
  }
  return fallback;
}

/** True for the 404 that means the id in the URL names nothing. */
export function isNotFound(caught: unknown): boolean {
  return caught instanceof ApiError && caught.status === 404;
}

/**
 * Logs a failure that is neither an answer from the server nor a lost
 * connection, which is to say a bug in this frontend. Called once where the
 * failure is caught — a `catch` block or a mutation's `onError` — and never
 * from render, where a formatter would log again on every keystroke.
 */
export function logUnexpected(caught: unknown): void {
  if (
    caught instanceof ApiError ||
    caught instanceof MessageError ||
    isNetworkFailure(caught)
  ) {
    return;
  }
  console.error(caught);
}
