// The helper layer every end-to-end scenario arranges through
// (CLAUDE.md, "Testing expectations": Playwright helpers live here).
//
// The implementations are split by concern — the stack's environment, REST,
// git, the orchestrator log, the browser, and the resource factories — and
// re-exported here so a spec has one import and the documented path stays the
// documented path.
//
// The one thing a spec does *not* get from here is `test` and `expect`: those
// come from `./fixtures`, the `test.extend` that supplies the `user`, `api`,
// `repo`, `project` and `sessions` fixtures a scenario arranges with.

export {
  apiBaseUrl,
  baseUrl,
  dataDir,
  engineBinary,
  orchestratorLogPath,
  randomSuffix,
  reposDir,
  sleep,
  stubImage,
  uniqueName,
  waitFor,
  type WaitForOptions,
} from "./env";

export {
  api,
  // The same factory under the name a spec uses when the `api` fixture already
  // holds that identifier (`utils/fixtures.ts`).
  api as apiClient,
  ApiCallError,
  createTestUser,
  currentUser,
  DEFAULT_TEST_PASSWORD,
  REFRESH_COOKIE_NAME,
  type Api,
  type CallOptions,
  type CreateTestUserOptions,
  type RawResponse,
  type TestUser,
} from "./api";

export {
  commitInSessionWorkClone,
  commitToBareRepo,
  createBareRepo,
  gitIsAncestor,
  gitRevParse,
  mirrorPath,
  sessionWorkPath,
  type BareRepo,
  type CreateBareRepoOptions,
  createBareRepoAt,
  gitLogLast,
} from "./git";

export { sessionContainers } from "./engine";

export { logOffset, readLoggedLink, type LoggedLinkKind } from "./log";

export { login, loginViaToken, newLoggedInPage } from "./browser";

export { armSocketDrop, dropConnection } from "./browser";

export { closeSockets } from "./browser";

export {
  createProject,
  createTask,
  defaultProfile,
  getTask,
  launchSession,
  listBranches,
  moveTask,
  sendInput,
  setProfileSecrets,
  setProjectSecret,
  waitForSessionState,
  type CreateProjectOptions,
  type LaunchSessionOptions,
} from "./resources";

export {
  endSession,
  setProfileIdleTimeout,
  waitForContainerRemoved,
} from "./resources";

export { loggedEmail } from "./log";

export {
  STREAMING_CURSOR,
  SUBAGENT_CHILDREN,
  TASK_COLUMN_PREFIX,
  TRANSCRIPT_SCROLL,
  taskCardTestId,
  taskColumnTestId,
} from "./test-ids";

export {
  openRow,
  pinToLatest,
  reveal,
  rowCount,
  transcript,
} from "./transcript";
