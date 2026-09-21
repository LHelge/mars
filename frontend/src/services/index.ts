export {
  ApiError,
  apiDelete,
  apiGet,
  apiPatch,
  apiPost,
  apiPut,
  onForbidden,
  onPasswordChangeRequired,
} from "./apiClient";

export {
  errorMessage,
  GENERIC_FAILURE,
  isNetworkFailure,
  isNotFound,
  logUnexpected,
  MessageError,
  statusMessage,
  UNREACHABLE,
} from "./errorMessage";

export {
  acceptInvite,
  clearAuth,
  getAccessToken,
  getAuthState,
  getCurrentUser,
  installSession,
  isAuthenticated,
  isStaleRefreshError,
  login,
  logout,
  lookupInvite,
  onCredentialsReplaced,
  onSignOut,
  refreshAccessToken,
  requestPasswordReset,
  resetPassword,
  setCurrentUser,
  signOut,
  StaleRefreshError,
  subscribe,
} from "./auth";
export type {
  AuthState,
  CredentialsReplacedHandler,
  InstallReason,
  SignOutHandler,
  SignOutReason,
} from "./auth";

export { getHealth } from "./health";
export type { Health } from "./health";

export {
  clearSharedDir,
  createProject,
  createSharedDir,
  deleteProject,
  deleteSharedDir,
  fetchProject,
  getProject,
  listBranches,
  listProjects,
  listSharedDirs,
  retryClone,
  updateProject,
} from "./projects";

export {
  createProfile,
  deleteProfile,
  getProfile,
  listProfiles,
  updateProfile,
} from "./profiles";

export {
  getDiff,
  isGitConflict,
  listSessionBranches,
  merge,
  push,
  rebase,
} from "./git";

export {
  createSecret,
  deleteSecret,
  listSecretUses,
  listSecrets,
  patchSecret,
  replaceSecretValue,
} from "./secrets";
export type { ListSecretsParams } from "./secrets";

export { queryKeys } from "./queryKeys";

export {
  createSession,
  deleteSession,
  endSession,
  getSession,
  listEvents,
  listProjectSessions,
  listSessionTasks,
  listSessions,
  retrySession,
  sendInput,
  stopSession,
  syncSession,
  updateSession,
} from "./sessions";
export type { ListEventsParams, ListSessionsParams } from "./sessions";

export {
  addComment,
  addDependency,
  createTask,
  deleteTask,
  getTask,
  listHumanTasks,
  listTasks,
  releaseTask,
  removeDependency,
  taskStreamUrl,
  updateTask,
} from "./tasks";
export type { TaskFilters, TaskRef } from "./tasks";

export {
  createTaskState,
  deleteTaskState,
  listTaskStates,
  updateTaskState,
} from "./taskStates";

export {
  changePassword,
  createInvite,
  deleteUser,
  getMe,
  getUser,
  listInvites,
  listUsers,
  resendInvite,
  revokeInvite,
  updateMe,
  updateUser,
} from "./users";
