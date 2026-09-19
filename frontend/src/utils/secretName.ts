// The name rule of `docs/data-model.md`, `secrets`: `^[A-Z][A-Z0-9_]{0,127}$`.
// A secret name becomes an environment variable in a session container, which
// is why it is an environment-variable name and not free text. Checking it in
// the browser is a courtesy, never the authority: the orchestrator applies the
// same rule and answers 400 for anything that slips past.

export const SECRET_NAME_RE = /^[A-Z][A-Z0-9_]{0,127}$/;

/** The one message the form shows for every shape of a bad name. */
export const SECRET_NAME_MESSAGE =
  "Name must be an environment-variable name: uppercase letters, digits and underscores, starting with a letter";

/** The message for a name that cannot be stored, or `null` when it can. */
export function validateSecretName(name: string): string | null {
  return SECRET_NAME_RE.test(name) ? null : SECRET_NAME_MESSAGE;
}
