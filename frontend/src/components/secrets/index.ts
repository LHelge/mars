// The secrets manager and its parts (`SPEC.md`, "User-facing features",
// Secrets). `/secrets` and the project page's secrets tab both mount
// `SecretsManager`; the rest are its internals, exported for tests.

export { CreateSecretForm } from "./CreateSecretForm";
export type { CreateSecretFormProps } from "./CreateSecretForm";

export { SecretRow } from "./SecretRow";
export type { SecretRowProps } from "./SecretRow";

export { SecretsManager } from "./SecretsManager";
export type { SecretsManagerProps } from "./SecretsManager";

export {
  SecretUsesList,
  USES_INITIAL_LIMIT,
  USES_MAX_LIMIT,
} from "./SecretUsesList";
export type { SecretUsesListProps } from "./SecretUsesList";

export {
  DUPLICATE_SECRET_MESSAGE,
  FORBIDDEN_SECRET_MESSAGE,
  PRECEDENCE_HELP,
  secretErrorMessage,
} from "./messages";
