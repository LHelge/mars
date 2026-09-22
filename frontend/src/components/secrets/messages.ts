// The fixed strings of the secrets manager, and the mapping from a failed
// request to the one the operator sees. One place, because the create form,
// the rename field, the value replacement, the flag toggle and the delete all
// answer the same four cases.

import { ApiError } from "../../services/apiClient";
import { errorMessage } from "../../services/errorMessage";

/**
 * `ARCHITECTURE.md`, "Secrets", Resolution at launch: which secrets reach a
 * session at all, and which scope wins a name. Agent credentials are the one
 * exception to the first half — injected without being listed (ADR 0036) —
 * which matters on a project's tab, where they are in the same table.
 */
export const PRECEDENCE_HELP =
  "A secret reaches a session only when the session's profile lists its name; agent credentials need no listing. Where the name is set at several scopes, the launching user's secret overrides the project's, which overrides the global one. Orchestrator-only secrets are never injected.";

/**
 * The name rule of `utils/secretName.ts` (`docs/data-model.md`, `secrets`:
 * `^[A-Z][A-Z0-9_]{0,127}$`) in words, before anything is typed.
 */
export const SECRET_NAME_HINT =
  "Environment variable name: A–Z, 0–9 and _, starting with a letter.";

/**
 * What the orchestrator-only flag does (`ARCHITECTURE.md`, "Secrets",
 * Resolution at launch): a winning row with the flag is skipped, and no
 * lower-precedence row takes its place.
 */
export const ORCHESTRATOR_ONLY_HINT =
  "Kept for Mars itself (e.g. git); never injected into a session, even if a profile declares it.";

/** The flag's short form, beside a row that carries it. */
export const ORCHESTRATOR_ONLY_BADGE = "never injected";

/**
 * The project's git credential (`SPEC.md`, "Projects"): what it is for and
 * the two things to know about keeping it.
 */
export const GIT_CREDENTIAL_NOTE =
  "Used by git operations for this project. Replace it when the token expires; keep it orchestrator-only.";

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
