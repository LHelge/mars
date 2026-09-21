// Mirrors `SPEC.md`, "Secrets (`/api/secrets`)"; `SecretScope` is the
// `secret_scope` enum of `docs/data-model.md`, "Enums". No response carries `value`.

export type SecretScope = "global" | "user" | "project";

/**
 * An agent backend, as `SPEC.md`, "Secrets", spells it. v1 has one; a second
 * one adds its name here and its credential table in `src/secrets/`.
 */
export type AgentBackend = "claude";

export interface SecretMeta {
  id: string;
  scope: SecretScope;
  scope_id: string | null;
  name: string;
  orchestrator_only: boolean;
  key_version: number;
  created_by: string | null;
  created_at: string;
  updated_at: string;
  last_used_at: string | null;
  /**
   * The backend whose agent credential this name is, else null (`SPEC.md`,
   * "Secrets"): derived from the name by the server, never stored.
   */
  credential_for: AgentBackend | null;
}

/** A row of `GET /secrets/{id}/uses`. */
export interface SecretUse {
  session_id: string | null;
  user_id: string | null;
  purpose: "launch" | "git";
  at: string;
}

export interface CreateSecretRequest {
  scope: SecretScope;
  scope_id?: string;
  name: string;
  value: string;
  orchestrator_only?: boolean;
}

export interface ReplaceSecretRequest {
  value: string;
}

export interface PatchSecretRequest {
  name?: string;
  orchestrator_only?: boolean;
}
