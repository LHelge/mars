// The fixed strings of the secrets manager, and the mapping from a failed
// request to the one the operator sees. One place, because the create form,
// the rename field, the value replacement, the flag toggle and the delete all
// answer the same four cases.

import { ApiError } from "../../services/apiClient";

/** `ARCHITECTURE.md`, "Secrets": the resolution order, in one line. */
export const PRECEDENCE_HELP =
  "At launch, project secrets override global ones and your user secrets override both; orchestrator-only secrets are never injected.";

/** 409 from a create or a rename: the scope already has that name. */
export const DUPLICATE_SECRET_MESSAGE =
  "A secret with that name already exists in this scope.";

/**
 * 403 on a user-scoped read or mutation (`SPEC.md`, "Secrets": user-scoped
 * secrets are the owner's or an administrator's).
 */
export const FORBIDDEN_SECRET_MESSAGE =
  "You can only manage your own secrets.";

/** A failure that is neither the server's fault to explain nor ours to guess. */
const FALLBACK_MESSAGE = "Something went wrong";

/**
 * A validation 400 is shown as the server wrote it — it names the field. A
 * 500, including the one a rename's re-encryption could raise, stays generic
 * and is never retried automatically.
 */
export function secretErrorMessage(caught: unknown): string {
  if (caught instanceof ApiError) {
    if (caught.status === 409) {
      return DUPLICATE_SECRET_MESSAGE;
    }
    if (caught.status === 403) {
      return FORBIDDEN_SECRET_MESSAGE;
    }
    if (caught.status < 500) {
      return caught.error;
    }
  }
  console.error(caught);
  return FALLBACK_MESSAGE;
}

/** True when a list failed because the caller may not see that user's scope. */
export function isForbidden(caught: unknown): boolean {
  return caught instanceof ApiError && caught.status === 403;
}
