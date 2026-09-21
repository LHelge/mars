// The fixed strings of the secrets manager, and the mapping from a failed
// request to the one the operator sees. One place, because the create form,
// the rename field, the value replacement, the flag toggle and the delete all
// answer the same four cases.

import { ApiError } from "../../services/apiClient";
import { errorMessage } from "../../services/errorMessage";

/** `ARCHITECTURE.md`, "Secrets": the resolution order, in one line. */
export const PRECEDENCE_HELP =
  "At launch, project secrets override global ones and your user secrets override both; orchestrator-only secrets are never injected.";

/** 409 from a create or a rename: the scope already has that name. */
export const DUPLICATE_SECRET_MESSAGE =
  "A secret with that name already exists in this scope.";

/**
 * The exact body of the duplicate-name 409 (`repositories/secrets.rs`, the
 * unique index on scope, `scope_id` and name). The other 409 of these
 * endpoints — `this scope already has an agent credential (<NAME>); replace or
 * delete it first` — names what is in the way and what to do about it, so it
 * is shown as the server wrote it (`SPEC.md`, "Secrets" and "Frontend",
 * Failure messages). Keyed on the prose because the status cannot tell the two
 * apart; a reword on the server falls back to the server's own words.
 */
const DUPLICATE_SECRET_BODY = "secret already exists";

/**
 * 403 on a user-scoped read or mutation (`SPEC.md`, "Secrets": user-scoped
 * secrets are the owner's or an administrator's).
 */
export const FORBIDDEN_SECRET_MESSAGE =
  "You can only manage your own secrets.";

/**
 * The two cases this manager words itself; everything else — including the
 * validation 400 and the agent-credential 409, both shown as the server wrote
 * them because they name what is wrong — is `errorMessage`'s single rule
 * (`services/errorMessage.ts`).
 */
export function secretErrorMessage(caught: unknown): string {
  if (caught instanceof ApiError) {
    if (caught.status === 409 && caught.error.trim() === DUPLICATE_SECRET_BODY) {
      return DUPLICATE_SECRET_MESSAGE;
    }
    if (caught.status === 403) {
      return FORBIDDEN_SECRET_MESSAGE;
    }
  }
  return errorMessage(caught);
}

/** True when a list failed because the caller may not see that user's scope. */
export function isForbidden(caught: unknown): boolean {
  return caught instanceof ApiError && caught.status === 403;
}
