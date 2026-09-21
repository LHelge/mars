// The factories scenarios arrange with: a project that has finished cloning, a
// profile carrying the stub's knobs, a launched session in a known state, a
// task on the board. Each waits for the state its documented contract promises
// and throws with what it last saw, so a failing run is diagnosable from the
// Playwright report without a rerun.

import type {
  Branch,
  TaskComment,
  CreateTaskInput,
  Profile,
  ProfileInput,
  Project,
  SecretMeta,
  Session,
  SessionCreateInput,
  SessionState,
  Task,
  TaskDetail,
} from "../../src/types";
import type { Api } from "./api";
import { uniqueName, waitFor } from "./env";

// --- projects ---------------------------------------------------------------

export interface CreateProjectOptions {
  name?: string;
  remote_url: string;
  default_branch?: string;
}

/** How long a `file://` clone is given before the wait is called a failure. */
const CLONE_TIMEOUT_MS = 60_000;
const CLONE_POLL_MS = 500;

/**
 * Creates a project and returns it once the clone has finished (`SPEC.md`,
 * "Projects": `POST` answers 201 with `status: cloning`, the row reaches
 * `ready` or `error`). An `error` throws at once with `status_message` instead
 * of polling to the timeout — a misconfigured engine or git is not something
 * waiting fixes.
 */
export async function createProject(
  client: Api,
  opts: CreateProjectOptions,
): Promise<Project> {
  const created = await client.post<Project>("/projects", {
    name: opts.name ?? uniqueName("e2e-project"),
    remote_url: opts.remote_url,
    ...(opts.default_branch === undefined
      ? {}
      : { default_branch: opts.default_branch }),
  });

  return waitFor(
    async () => {
      const project = await client.get<Project>(`/projects/${created.id}`);
      if (project.status === "error") {
        throw new Error(
          `project ${project.id} (${project.remote_url}) failed to clone: ${
            project.status_message ?? "no status_message"
          }`,
        );
      }
      return project.status === "ready" ? project : null;
    },
    {
      timeoutMs: CLONE_TIMEOUT_MS,
      intervalMs: CLONE_POLL_MS,
      description: `project ${created.id} to reach status ready`,
    },
  );
}

/**
 * Every session of a project (`SPEC.md`, "Sessions"), whoever launched it.
 *
 * What the `sessions` fixture's sweep reads: a session the dispatcher started
 * has no launch call to register for clean-up, so the only way to find it is
 * to ask the project (`tests/utils/fixtures.ts`).
 */
export function listProjectSessions(
  client: Api,
  projectId: string,
): Promise<Session[]> {
  return client.get<Session[]>(`/projects/${projectId}/sessions`);
}

/**
 * The project's automation pause (`SPEC.md`, "Projects": `PUT` with
 * `automation_paused`), which stops every unattended launch there while it is
 * set and refuses nothing a person does.
 */
export function setAutomationPaused(
  client: Api,
  projectId: string,
  paused: boolean,
): Promise<Project> {
  return client.put<Project>(`/projects/${projectId}`, {
    automation_paused: paused,
  });
}

/** The project's branches (`SPEC.md`, "Projects"). */
export function listBranches(
  client: Api,
  projectId: string,
): Promise<Branch[]> {
  return client.get<Branch[]>(`/projects/${projectId}/branches`);
}

// --- profiles and secrets ---------------------------------------------------

/** The project's default profile, the one sessions launch with when none is named. */
export async function defaultProfile(
  client: Api,
  projectId: string,
): Promise<Profile> {
  const profiles = await client.get<Profile[]>(
    `/projects/${projectId}/profiles`,
  );
  const profile = profiles.find((candidate) => candidate.is_default);
  if (!profile) {
    throw new Error(
      `project ${projectId} has no default profile (${profiles.length} profiles)`,
    );
  }
  return profile;
}

/** The whole profile as a `PUT` body: `PUT` replaces, so nothing may be dropped. */
function toProfileInput(profile: Profile): ProfileInput {
  return {
    name: profile.name,
    kind: profile.kind,
    backend: profile.backend,
    model: profile.model,
    system_prompt: profile.system_prompt,
    permission_mode: profile.permission_mode,
    image: profile.image,
    runtime: profile.runtime,
    mcp_tools: profile.mcp_tools,
    secrets: profile.secrets,
    serves_states: profile.serves_states,
    partial_messages: profile.partial_messages,
    idle_timeout_secs: profile.idle_timeout_secs,
  };
}

/**
 * Declares `names` on the project's default profile. Together with
 * [`setProjectSecret`] this is how a scenario passes the stub's knobs —
 * `MARS_STUB_LINE_DELAY_MS`, `MARS_STUB_EXIT_AFTER_TURNS`,
 * `MARS_STUB_EXIT_CODE`, `MARS_STUB_FIXTURE` — into the container: a declared
 * secret is resolved into the launcher's environment (ARCHITECTURE.md,
 * "Session image").
 */
export async function setProfileSecrets(
  client: Api,
  projectId: string,
  names: string[],
): Promise<Profile> {
  const profile = await defaultProfile(client, projectId);
  return client.put<Profile>(`/projects/${projectId}/profiles/${profile.id}`, {
    ...toProfileInput(profile),
    secrets: names,
  });
}

/**
 * The agent credential every test user is given, obviously fake (`CLAUDE.md`,
 * rule 3). The name is the CLI's — `agentCredentials.ts` on the other side
 * spells the same one — and the value authenticates nothing: the stub image
 * never calls a model.
 */
export const FAKE_AGENT_CREDENTIAL = "fake-oauth-token-for-tests";

/**
 * Gives `client`'s own user scope a Claude credential, so a launch form shows
 * the ordinary `Authenticates with your …` line and its button still says
 * `Launch session` (`SPEC.md`, "Frontend", Agent credentials). Every scenario
 * gets one through the `api` fixture; a spec that is *about* the empty state
 * opts out with `test.use({ agentCredential: false })`.
 */
export function seedAgentCredential(client: Api): Promise<SecretMeta> {
  return client.post<SecretMeta>("/secrets", {
    scope: "user",
    name: "CLAUDE_CODE_OAUTH_TOKEN",
    value: FAKE_AGENT_CREDENTIAL,
  });
}

/** A project-scoped secret (`SPEC.md`, "Secrets"); the value never comes back. */
export function setProjectSecret(
  client: Api,
  projectId: string,
  name: string,
  value: string,
): Promise<SecretMeta> {
  return client.post<SecretMeta>("/secrets", {
    scope: "project",
    scope_id: projectId,
    name,
    value,
  });
}

// --- sessions ---------------------------------------------------------------

export type LaunchSessionOptions = Partial<SessionCreateInput>;

/**
 * Launches a session (`SPEC.md`, "Sessions": 201 with `state: creating`).
 * Without `profile_id` the project's default profile is used.
 */
export async function launchSession(
  client: Api,
  projectId: string,
  opts: LaunchSessionOptions = {},
): Promise<Session> {
  const profileId =
    opts.profile_id ?? (await defaultProfile(client, projectId)).id;
  return client.post<Session>(`/projects/${projectId}/sessions`, {
    ...opts,
    profile_id: profileId,
  });
}

/**
 * Polls `GET /sessions/{id}` until the session is in one of `states`. A session
 * that stops somewhere else keeps being polled — only the timeout ends the
 * wait, and it names the last state and error seen.
 */
export async function waitForSessionState(
  client: Api,
  sessionId: string,
  states: SessionState | SessionState[],
  timeoutMs = 60_000,
): Promise<Session> {
  const wanted = Array.isArray(states) ? states : [states];
  // A holder, so the last observation survives the timeout for the error text.
  const observed: { session: Session | null } = { session: null };

  try {
    return await waitFor(
      async () => {
        const session = await client.get<Session>(`/sessions/${sessionId}`);
        observed.session = session;
        return wanted.includes(session.state) ? session : null;
      },
      {
        timeoutMs,
        intervalMs: 250,
        description: `session ${sessionId} to reach ${wanted.join(" or ")}`,
      },
    );
  } catch (error) {
    const seen = observed.session;
    if (seen === null) throw error;
    throw new Error(
      `${(error as Error).message}; last state was ${seen.state}` +
        (seen.error === null ? "" : ` (error: ${seen.error})`),
      { cause: error },
    );
  }
}

/** `POST /sessions/{id}/input` (`SPEC.md`, "Sessions"): accepted with 202. */
export async function sendInput(
  client: Api,
  sessionId: string,
  text: string,
): Promise<void> {
  const result = await client.send("POST", `/sessions/${sessionId}/input`, {
    kind: "message",
    text,
  });
  if (result.status !== 202) {
    throw new Error(
      `POST /sessions/${sessionId}/input answered ${result.status}, not 202: ${result.text}`,
    );
  }
}

/**
 * Sets the default profile's `idle_timeout_secs`. The model's floor is one
 * second (`orchestrator/src/models/agent_profile.rs`), and the idle reaper is a
 * cron job on a 60 s period, so a session launched under a one-second timeout
 * is parked at the next tick — within about a minute (`ARCHITECTURE.md`,
 * "Session owner task", point 4; "Background jobs").
 */
export async function setProfileIdleTimeout(
  client: Api,
  projectId: string,
  seconds: number,
): Promise<Profile> {
  const profile = await defaultProfile(client, projectId);
  return client.put<Profile>(`/projects/${projectId}/profiles/${profile.id}`, {
    ...toProfileInput(profile),
    idle_timeout_secs: seconds,
  });
}

/**
 * `POST /sessions/{id}/end`, tolerating a session that is already `done` or
 * gone. This is the clean-up every session scenario ends with: a container left
 * running outlives the test, and `tests/e2e-stack.sh down` would be the only
 * thing to remove it.
 *
 * A session that is still `creating` is ended as it is: the orchestrator
 * cancels its launch and removes whatever container it had got as far as
 * creating (`SPEC.md`, "Sessions"; task `qhyhw`), so there is nothing to wait
 * out and nothing left behind.
 */
export async function endSession(
  client: Api,
  sessionId: string,
): Promise<void> {
  await client.send("POST", `/sessions/${sessionId}/end`, undefined, {
    allow: [404, 409],
  });
}

/**
 * Waits until the session row has no `container_id`, the documented rest state
 * of `parked` and `done` (`ARCHITECTURE.md`, "Session lifecycle": the container
 * is removed). The owner clears it after the state change, so a scenario that
 * acts on the state alone is acting while the owner is still finishing.
 */
export function waitForContainerRemoved(
  client: Api,
  sessionId: string,
  timeoutMs = 30_000,
): Promise<Session> {
  return waitFor(
    async () => {
      const session = await client.get<Session>(`/sessions/${sessionId}`);
      return session.container_id === null ? session : null;
    },
    {
      timeoutMs,
      intervalMs: 200,
      description: `session ${sessionId} to have its container removed`,
    },
  );
}

// --- tasks ------------------------------------------------------------------

export function createTask(
  client: Api,
  projectId: string,
  input: CreateTaskInput,
): Promise<Task> {
  return client.post<Task>(`/projects/${projectId}/tasks`, input);
}

/** `GET /projects/{pid}/tasks/{id}`, by UUID or per-project number. */
export function getTask(
  client: Api,
  projectId: string,
  idOrNumber: string | number,
): Promise<TaskDetail> {
  return client.get<TaskDetail>(`/projects/${projectId}/tasks/${idOrNumber}`);
}

/**
 * `POST /projects/{pid}/tasks/{id}/comments` — a change to the task made from
 * outside the browser, which reaches an open drawer as a `commented` event.
 */
export function commentOnTask(
  client: Api,
  projectId: string,
  idOrNumber: string | number,
  body: string,
): Promise<TaskComment> {
  return client.post<TaskComment>(
    `/projects/${projectId}/tasks/${idOrNumber}/comments`,
    { body },
  );
}

/** Moves a task to another state, the board's one mutation. */
export function moveTask(
  client: Api,
  projectId: string,
  idOrNumber: string | number,
  state: string,
): Promise<Task> {
  return client.put<Task>(`/projects/${projectId}/tasks/${idOrNumber}`, {
    state,
  });
}
