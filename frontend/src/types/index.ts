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
export type {
  Branch,
  BranchKind,
  Project,
  ProjectCreateInput,
  ProjectStatus,
  ProjectUpdateInput,
  SharedDir,
  SharedDirInput,
} from "./projects";
export type { Profile, ProfileInput, ProfileKind } from "./profiles";
export { PROFILE_GATED_TOOLS } from "./profiles";
export type {
  EventsPage,
  Session,
  SessionCreateInput,
  SessionInput,
  SessionKind,
  SessionState,
  SyncResult,
} from "./sessions";
export type {
  AgentEvent,
  AgentEventBase,
  AgentEventKind,
  GitDetail,
  GitOp,
  McpServerStatus,
} from "./agentEvent";
export type {
  ClientMessage,
  ClientMessageType,
  ServerMessage,
  ServerMessageType,
} from "./sessionSocket";
export type {
  CommitResult,
  Diff,
  DiffFile,
  DiffTarget,
  GitConflictError,
  MergeInput,
  PushInput,
  PushResult,
  RebaseInput,
  SessionBranch,
} from "./git";
export type { TaskState, TaskStateKind } from "./taskStates";
export type {
  Comment,
  Handoff,
  ReviewStatus,
  Task,
  TaskDependency,
  TaskDependencyKind,
  TaskDetail,
  TaskSessionTouch,
} from "./tasks";
