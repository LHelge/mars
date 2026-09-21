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
  AgentBackend,
  AgentCredential,
  AgentCredentialStatus,
  CreateSecretRequest,
  PatchSecretRequest,
  SecretMeta,
  SecretScope,
  SecretUse,
} from "./secrets";
export type {
  Branch,
  Project,
  ProjectCreateInput,
  ProjectStatus,
  ProjectUpdateInput,
  SharedDir,
  SharedDirInput,
} from "./projects";
export type {
  Profile,
  ProfileInput,
  ProfileKind,
  ProfileTemplate,
} from "./profiles";
export { PROFILE_GATED_TOOLS } from "./profiles";
export type {
  EventsPage,
  LaunchSource,
  Session,
  SessionCreateInput,
  SessionInput,
  SessionKind,
  SessionState,
  SyncResult,
} from "./sessions";
export type { AgentEvent, AgentEventKind, GitDetail } from "./agentEvent";
export type { ClientMessage, ServerMessage } from "./sessionSocket";
export type {
  CommitResult,
  Diff,
  DiffTarget,
  MergeInput,
  PushInput,
  PushResult,
  RebaseInput,
  SessionBranch,
} from "./git";
export type {
  CreateTaskStateInput,
  TaskState,
  TaskStateKind,
  UpdateTaskStateInput,
} from "./taskStates";
export type {
  CreateTaskInput,
  ForwardHandoffInput,
  Handoff,
  HandoffInput,
  ReviewStatus,
  RevisionHandoffInput,
  Task,
  TaskDependency,
  TaskDependencyKind,
  TaskComment,
  TaskDetail,
  TaskEvent,
  TaskEventKind,
  TaskPriority,
  TaskSessionTouch,
  UpdateTaskInput,
} from "./tasks";
