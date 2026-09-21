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

/**
 * The credential half of one [`AgentCredentialStatus`] (`SPEC.md`, "Secrets"):
 * which secret row would be injected, under which of the backend's names, from
 * which scope. Structurally valueless — the preflight decrypts nothing — and
 * carries no `scope_id`, because the answer is already per caller and per
 * project.
 */
export interface AgentCredential {
  secret_id: string;
  name: string;
  scope: SecretScope;
}

/**
 * One entry of `GET /projects/{pid}/agent-credentials` (`SPEC.md`, "Secrets"):
 * which credential a session of that project launched by *the caller* would be
 * given, per backend, or `null` when there is none at any scope.
 */
export interface AgentCredentialStatus {
  backend: AgentBackend;
  credential: AgentCredential | null;
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
