// Typed wrappers for the endpoints of `SPEC.md`, "Secrets (`/api/secrets`)",
// including the project-scoped agent-credential preflight that section
// documents. No response carries `value`, so nothing here ever answers
// a plaintext: a value only travels outwards, in the body of a create or a
// replace, and the caller drops it as soon as the promise settles
// (`CLAUDE.md`, rule 3).

import type {
  AgentCredentialStatus,
  CreateSecretRequest,
  PatchSecretRequest,
  SecretMeta,
  SecretScope,
  SecretUse,
} from "../types";
import { apiDelete, apiGet, apiPatch, apiPost, apiPut, seg } from "./apiClient";

export interface ListSecretsParams {
  scope: SecretScope;
  /** The project for `project`, the user for `user`; omitted means "mine". */
  scope_id?: string;
}

/**
 * The metadata of one scope. `user` without a `scope_id` is the caller's own;
 * another user's is an administrator's read and 403 for everyone else.
 */
export function listSecrets(params: ListSecretsParams): Promise<SecretMeta[]> {
  return apiGet<SecretMeta[]>("/secrets", {
    query: { scope: params.scope, scope_id: params.scope_id },
  });
}

/** 201 with the new metadata; 409 when the scope already has that name. */
export function createSecret(body: CreateSecretRequest): Promise<SecretMeta> {
  return apiPost<SecretMeta>("/secrets", body);
}

/** Replaces the stored value; the metadata comes back with a new `updated_at`. */
export function replaceSecretValue(
  id: string,
  value: string,
): Promise<SecretMeta> {
  return apiPut<SecretMeta>(`/secrets/${seg(id)}`, { value });
}

/** Rename and the orchestrator-only flag; a rename re-encrypts on the server. */
export function patchSecret(
  id: string,
  body: PatchSecretRequest,
): Promise<SecretMeta> {
  return apiPatch<SecretMeta>(`/secrets/${seg(id)}`, body);
}

export function deleteSecret(id: string): Promise<void> {
  return apiDelete(`/secrets/${seg(id)}`);
}

/**
 * Which agent credential a session of this project, launched by the caller,
 * would authenticate with — one entry per backend, `credential: null` when
 * there is none (`SPEC.md`, "Secrets": 404 for an unknown project). It
 * decrypts nothing and writes no use, so a page may read it as often as it
 * likes.
 */
export function getAgentCredentials(
  projectId: string,
): Promise<AgentCredentialStatus[]> {
  return apiGet<AgentCredentialStatus[]>(
    `/projects/${seg(projectId)}/agent-credentials`,
  );
}

/** The audit trail, newest first. The server caps `limit` at 500. */
export function listSecretUses(
  id: string,
  limit = 20,
): Promise<SecretUse[]> {
  return apiGet<SecretUse[]>(`/secrets/${seg(id)}/uses`, { query: { limit } });
}
