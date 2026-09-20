// The helper layer every end-to-end scenario arranges through
// (CLAUDE.md, "Testing expectations": Playwright helpers live here).
//
// The implementations are split by concern — the stack's environment, REST,
// git, the orchestrator log, the browser, and the resource factories — and
// re-exported here so a spec has one import and the documented path stays the
// documented path.

export {
  apiBaseUrl,
  baseUrl,
  dataDir,
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
} from "./git";

export { logOffset, readLoggedLink, type LoggedLinkKind } from "./log";

export { login, loginViaToken, newLoggedInPage } from "./browser";

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
