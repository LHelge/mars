// Barrel for the API shape mirrors of `SPEC.md`, "REST API".
//
// Mostly types, imported with `import type` (`verbatimModuleSyntax`). The few
// values here belong to a type rather than beside it: the `const` array a union
// is derived from, when a picker renders the same list, and the
// `parseX(value): X | undefined` that reads a DOM string back into it
// (`ARCHITECTURE.md`, "Frontend architecture", Types at the edges).

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
export { AGENT_BACKENDS } from "./secrets";
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
  PermissionMode,
  Profile,
  ProfileGatedTool,
  ProfileInput,
  ProfileKind,
  ProfileTemplate,
} from "./profiles";
export { parseProfileKind, PROFILE_GATED_TOOLS } from "./profiles";
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
export type {
  AgentEvent,
  AgentEventKind,
  GitDetail,
  WorkTreeOutcome,
} from "./agentEvent";
export type { ClientMessage, ServerMessage } from "./sessionSocket";
export type {
  CommitResult,
  Diff,
  DiffStatus,
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
export { parseTaskStateKind, TASK_STATE_KINDS } from "./taskStates";
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
export {
  parseTaskDependencyKind,
  parseTaskPriority,
  TASK_DEPENDENCY_KINDS,
  TASK_PRIORITIES,
} from "./tasks";
