// `SPEC.md`, "Agent profiles (`/api/projects/{pid}/profiles`)". `PUT` replaces
// the whole profile rather than patching it.

import type { Profile, ProfileInput, ProfileTemplate } from "../types";
import { apiDelete, apiGet, apiPost, apiPut } from "./apiClient";

/**
 * The four role templates (`SPEC.md`, "Role profile templates"), in that
 * section's order. Top-level rather than project-scoped: the answer does not
 * depend on the project, and it is the same for every build.
 */
export function listProfileTemplates(): Promise<ProfileTemplate[]> {
  return apiGet<ProfileTemplate[]>("/profile-templates");
}

/** Oldest first. */
export function listProfiles(pid: string): Promise<Profile[]> {
  return apiGet<Profile[]>(`/projects/${pid}/profiles`);
}

export function createProfile(
  pid: string,
  input: ProfileInput,
): Promise<Profile> {
  return apiPost<Profile>(`/projects/${pid}/profiles`, input);
}

export function updateProfile(
  pid: string,
  id: string,
  input: ProfileInput,
): Promise<Profile> {
  return apiPut<Profile>(`/projects/${pid}/profiles/${id}`, input);
}

/** 204; 409 for the project's default profile or one that has sessions. */
export function deleteProfile(pid: string, id: string): Promise<void> {
  return apiDelete(`/projects/${pid}/profiles/${id}`);
}
