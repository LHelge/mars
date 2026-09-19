// Barrel for the API shape mirrors of `SPEC.md`, "REST API".
// Every consumer imports these with `import type` (`verbatimModuleSyntax`).

export type { ApiErrorBody } from "./api";
export type {
  AcceptInviteRequest,
  AuthResponse,
  CreateInviteRequest,
  Invite,
  InviteLookup,
  LoginRequest,
  PasswordChangeRequest,
  UpdateMeRequest,
  UpdateUserRequest,
  User,
} from "./users";
export type {
  CreateSecretRequest,
  PatchSecretRequest,
  ReplaceSecretRequest,
  SecretMeta,
  SecretScope,
  SecretUse,
} from "./secrets";
export type { Project, ProjectStatus } from "./projects";
export type { Session, SessionKind, SessionState } from "./sessions";
export type {
  Comment,
  Handoff,
  ReviewStatus,
  Task,
  TaskDependency,
  TaskDependencyKind,
} from "./tasks";
