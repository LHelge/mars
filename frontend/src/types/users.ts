// Mirrors `SPEC.md`, "Auth (`/api/auth`)" and "Users (`/api/users`)".
// `User` and `Invite` are the shape lines of the "Users" section.

export interface User {
  id: string;
  username: string;
  email: string;
  admin: boolean;
  must_change_password: boolean;
  notify_email: boolean;
  created_at: string;
}

export interface Invite {
  id: string;
  email: string;
  admin: boolean;
  invited_by: string | null;
  expires_at: string;
  created_at: string;
}

export interface AuthResponse {
  user: User;
  access_token: string;
}

export interface LoginRequest {
  username: string;
  password: string;
}

export interface AcceptInviteRequest {
  token: string;
  username: string;
  password: string;
}

/** `GET /auth/invite/{token}` response. */
export interface InviteLookup {
  email: string;
  admin: boolean;
  expires_at: string;
}

/** `POST /users/{id}/password`; `current_password` is omitted by an admin. */
export interface PasswordChangeRequest {
  current_password?: string;
  password: string;
}

export interface UpdateMeRequest {
  notify_email?: boolean;
}

export interface UpdateUserRequest {
  username: string;
  admin: boolean;
}

export interface CreateInviteRequest {
  email: string;
  admin?: boolean;
}
