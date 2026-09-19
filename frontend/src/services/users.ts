// Typed wrappers for every `/api/users` endpoint of `SPEC.md`,
// "Users (`/api/users`)". Components never call `fetch`; every request goes
// through `apiClient` (`CLAUDE.md`, "Frontend conventions").

import type {
  CreateInviteRequest,
  AuthResponse,
  Invite,
  PasswordChangeRequest,
  UpdateMeRequest,
  UpdateUserRequest,
  User,
} from "../types";
import { apiDelete, apiGet, apiPatch, apiPost, apiPut } from "./apiClient";

/** The current user; the authenticated bootstrap's first read. */
export function getMe(): Promise<User> {
  return apiGet<User>("/users/me");
}

export function updateMe(body: UpdateMeRequest): Promise<User> {
  return apiPatch<User>("/users/me", body);
}

/** Admin only; ordered by `username`. */
export function listUsers(): Promise<User[]> {
  return apiGet<User[]>("/users");
}

export function getUser(id: string): Promise<User> {
  return apiGet<User>(`/users/${encodeURIComponent(id)}`);
}

/** Admin only; 409 when the change would remove the last administrator. */
export function updateUser(
  id: string,
  body: UpdateUserRequest,
): Promise<User> {
  return apiPut<User>(`/users/${encodeURIComponent(id)}`, body);
}

/** Admin only; 409 for the last administrator or for yourself. */
export function deleteUser(id: string): Promise<void> {
  return apiDelete(`/users/${encodeURIComponent(id)}`);
}

/**
 * Changing your own password answers `{user, access_token}` for the new
 * `auth_version`; an administrator changing someone else's answers 204, which
 * `apiClient` turns into `undefined`.
 */
export function changePassword(
  id: string,
  body: PasswordChangeRequest,
): Promise<AuthResponse | undefined> {
  return apiPost<AuthResponse | undefined>(
    `/users/${encodeURIComponent(id)}/password`,
    body,
  );
}

/** Admin only; open invites. The token is never part of the response. */
export function listInvites(): Promise<Invite[]> {
  return apiGet<Invite[]>("/users/invites");
}

/** Admin only; 409 when the email already has a user or an open invite. */
export function createInvite(body: CreateInviteRequest): Promise<Invite> {
  return apiPost<Invite>("/users/invites", body);
}

export function revokeInvite(id: string): Promise<void> {
  return apiDelete(`/users/invites/${encodeURIComponent(id)}`);
}

/** New token and expiry; the invitation email is sent again. */
export function resendInvite(id: string): Promise<Invite> {
  return apiPost<Invite>(`/users/invites/${encodeURIComponent(id)}/resend`);
}
