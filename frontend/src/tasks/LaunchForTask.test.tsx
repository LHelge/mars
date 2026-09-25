// What the drawer's launch does to the rest of the screen, and what it does
// not do once the user has left it (`SPEC.md`, "Frontend", "Task board"; the
// shared launch is `launch/useLaunchSession.ts`).
//
// Three rules, all of them about a request that outlives the click:
//
//   * A launch claims a task and creates a session, so the board's copy of the
//     task and the project's session list both go stale. Either one left alone
//     is a view that misses the launch until it polls.
//   * While the request is in flight there is nothing to cancel and no other
//     kind to switch to: the panel that owns the request stays put.
//   * A form that is gone when the answer arrives — the drawer closed on
//     Escape, the user went back to the board — is not pulled onto a session
//     page it is no longer waiting for.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { useEffect } from "react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { listProfiles } from "../services/profiles";
import { getProject, listBranches } from "../services/projects";
import { getAgentCredentials } from "../services/secrets";
import { createSession } from "../services/sessions";
import type {
  Profile,
  Project,
  Session,
  SessionCreateInput,
  TaskDetail,
} from "../types";
import { LaunchForTask } from "./LaunchForTask";
import { useTaskStore } from "./taskStore";

vi.mock("../services/profiles", () => ({ listProfiles: vi.fn() }));
vi.mock("../services/projects", () => ({
  getProject: vi.fn(),
  listBranches: vi.fn(),
}));
vi.mock("../services/secrets", () => ({ getAgentCredentials: vi.fn() }));
vi.mock("../services/sessions", () => ({ createSession: vi.fn() }));

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "00000000-0000-4000-8000-0000000000a1";
const TASK_ID = "00000000-0000-4000-8000-0000000000b2";
const PROFILE_ID = "00000000-0000-4000-8000-0000000000c3";
const SESSION_ID = "00000000-0000-4000-8000-0000000000d4";
const PLANNER_ID = "00000000-0000-4000-8000-0000000000c4";
const IMPLEMENTER_ID = "00000000-0000-4000-8000-0000000000c5";
const REVIEWER_ID = "00000000-0000-4000-8000-0000000000c6";
const NUMBER = 7;

function project(): Project {
  return {
    id: PROJECT_ID,
    name: "mars",
    remote_url: "git@example.invalid:acme/mars.git",
    default_branch: "main",
    status: "ready",
    status_message: null,
    last_fetched_at: null,
    max_attempts: 3,
    max_rounds: 5,
    max_concurrent_sessions: null,
    automation_paused: false,
    created_at: "2026-01-01T00:00:00Z",
    has_credential: true,
  };
}

/** The seeded default: conversational, serving no state (ADR 0051). */
function profile(): Profile {
  return {
    id: PROFILE_ID,
    project_id: PROJECT_ID,
    name: "claude",
    kind: "conversational",
    backend: "claude",
    image: "ghcr.io/example/mars-session:fake",
    model: null,
    system_prompt: "",
    serves_states: [],
    secrets: [],
    mcp_tools: [],
    runtime: null,
    permission_mode: "bypass",
    partial_messages: true,
    idle_timeout_secs: 900,
    is_default: true,
    auto_launch: false,
    max_concurrent: 1,
    schedule_cron: null,
    schedule_prompt: null,
    last_scheduled_at: null,
    next_scheduled_at: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
}

/** A seeded role: ephemeral, auto-launched, over one queue. */
function role(id: string, name: string, state: string): Profile {
  return {
    ...profile(),
    id,
    name,
    kind: "ephemeral",
    serves_states: [state],
    partial_messages: false,
    is_default: false,
    auto_launch: true,
  };
}

function task(): TaskDetail {
  return {
    id: TASK_ID,
    project_id: PROJECT_ID,
    number: NUMBER,
    title: "Teach the launch form one path",
    description: "",
    state: "ready",
    priority: 2,
    blocked: false,
    labels: [],
    parent_id: null,
    assignee_user_id: null,
    lease_holder_session_id: null,
    lease_since: null,
    attempts: 0,
    rounds: 0,
    needs_human_reason: null,
    handoff: null,
    depends_on: [],
    blocks: [],
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    closed_at: null,
    comments: [],
    handoffs: [],
    children: [],
    sessions: [],
  };
}

function session(): Session {
  return {
    id: SESSION_ID,
    project_id: PROJECT_ID,
    profile_id: PROFILE_ID,
    kind: "conversational",
    created_by: null,
    launch_source: "user",
    title: "Teach the launch form one path",
    task_id: TASK_ID,
    handoff_id: null,
    state: "creating",
    base_ref: "main",
    branch: null,
    container_id: null,
    cli_session_id: null,
    last_seq: 0,
    last_activity_at: "2026-01-01T00:00:00Z",
    cost_usd: 0,
    input_tokens: 0,
    output_tokens: 0,
    error: null,
    created_at: "2026-01-01T00:00:00Z",
    parked_at: null,
    ended_at: null,
  };
}

let client: QueryClient;
/** Where the router is, read after the answer rather than from a spy. */
let path: string;

function Probe() {
  const { pathname } = useLocation();
  // In an effect, not in render: the probe is a bystander and writes after the
  // render it observed has been committed.
  useEffect(() => {
    path = pathname;
  }, [pathname]);
  return null;
}

function mount() {
  client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const tree = (shown: boolean) => (
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/projects/${PROJECT_ID}`]}>
        <Probe />
        <Routes>
          <Route
            path="/projects/:id"
            element={
              shown ? (
                <LaunchForTask projectId={PROJECT_ID} task={task()} />
              ) : (
                <p>the board</p>
              )
            }
          />
          <Route path="/sessions/:id" element={<p>the session</p>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
  const rendered = render(tree(true));
  return {
    client,
    /** Take the form away without taking the router with it. */
    closeDrawer: () => {
      rendered.rerender(tree(false));
    },
  };
}

/** Open one of the two forms and wait for its profile list. */
async function openForm(action = "Open in session", option = /^claude/) {
  // The toggles are shut until the project has been read and is `ready`
  // (`tasks/launchRules.ts`).
  const toggle = screen.getByRole<HTMLButtonElement>("button", {
    name: action,
  });
  await waitFor(() => {
    expect(toggle.disabled).toBe(false);
  });
  fireEvent.click(toggle);
  const form = await screen.findByRole("form", { name: action });
  await within(form).findByRole("option", { name: option });
  return form;
}

/** A `createSession` that answers only when the test says so. */
function deferredLaunch() {
  let release: (value: Session) => void = () => undefined;
  vi.mocked(createSession).mockImplementation(
    () =>
      new Promise<Session>((resolve) => {
        release = resolve;
      }),
  );
  return () => {
    release(session());
  };
}

beforeEach(() => {
  path = "";
  useTaskStore.setState({ states: [] });
  vi.mocked(getProject).mockResolvedValue(project());
  vi.mocked(listProfiles).mockResolvedValue([profile()]);
  vi.mocked(listBranches).mockResolvedValue([
    { name: "main", kind: "head", commit: "a".repeat(40) },
  ]);
  vi.mocked(getAgentCredentials).mockResolvedValue([
    {
      backend: "claude",
      credential: {
        secret_id: "00000000-0000-4000-8000-0000000000e5",
        name: "CLAUDE_CODE_OAUTH_TOKEN",
        scope: "user",
      },
    },
  ]);
});

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

describe("LaunchForTask", () => {
  it("offers the seeded set a sensible launch for a task in ready", async () => {
    // A new project's profiles, in the order the API returns them.
    vi.mocked(listProfiles).mockResolvedValue([
      profile(),
      {
        ...profile(),
        id: PLANNER_ID,
        name: "planner",
        serves_states: ["backlog"],
      },
      role(IMPLEMENTER_ID, "implementer", "ready"),
      role(REVIEWER_ID, "reviewer", "review"),
    ]);
    vi.mocked(createSession).mockResolvedValue(session());
    mount();

    // Talking about the task: nobody conversational serves `ready`, so the
    // first conversational profile — the default `claude` — is offered.
    const talk = await openForm();
    expect(
      within(talk).getByLabelText<HTMLSelectElement>("Agent profile").value,
    ).toBe(PROFILE_ID);

    // Running it once: the ephemeral profile that serves `ready`.
    fireEvent.click(screen.getByRole("button", { name: "Run once" }));
    const run = await screen.findByRole("form", { name: "Run once" });
    await within(run).findByRole("option", {
      name: "implementer — serves ready",
    });
    expect(
      within(run).getByLabelText<HTMLSelectElement>("Agent profile").value,
    ).toBe(IMPLEMENTER_ID);
    expect(within(run).getByRole("option", { name: "reviewer" })).toBeDefined();

    fireEvent.click(within(run).getByRole("button", { name: "Run once" }));
    await waitFor(() => {
      expect(createSession).toHaveBeenCalledTimes(1);
    });
    expect(vi.mocked(createSession).mock.calls[0]?.[1]).toEqual({
      profile_id: IMPLEMENTER_ID,
      task_id: TASK_ID,
    } satisfies SessionCreateInput);
  });

  it("sends no base_ref of its own and settles both caches", async () => {
    vi.mocked(createSession).mockResolvedValue(session());

    const { client: queries } = mount();
    const invalidate = vi.spyOn(queries, "invalidateQueries");
    const form = await openForm();
    fireEvent.click(
      within(form).getByRole("button", { name: "Open in session" }),
    );

    await waitFor(() => {
      expect(createSession).toHaveBeenCalledTimes(1);
    });
    // Nothing the user did not choose: the server picks the base, which is how
    // a default branch changed a moment ago is still the one that applies
    // (`SPEC.md`, "Sessions").
    expect(vi.mocked(createSession).mock.calls[0]?.[1]).toEqual({
      profile_id: PROFILE_ID,
      task_id: TASK_ID,
    } satisfies SessionCreateInput);

    // The task the launch claimed and the project's session list, both.
    await waitFor(() => {
      expect(keysPassedTo(invalidate)).toContainEqual([
        "tasks",
        PROJECT_ID,
        String(NUMBER),
      ]);
    });
    expect(keysPassedTo(invalidate)).toContainEqual([
      "projects",
      PROJECT_ID,
      "sessions",
    ]);

    await waitFor(() => {
      expect(path).toBe(`/sessions/${SESSION_ID}`);
    });
  });

  it("shuts Cancel and the kind toggles while the launch is in flight", async () => {
    const answer = deferredLaunch();

    mount();
    const form = await openForm();
    fireEvent.click(
      within(form).getByRole("button", { name: "Open in session" }),
    );

    await waitFor(() => {
      expect(
        within(form).getByRole<HTMLButtonElement>("button", { name: "Cancel" })
          .disabled,
      ).toBe(true);
    });
    for (const name of ["Open in session", "Run once"]) {
      const toggle = screen
        .getAllByRole<HTMLButtonElement>("button", { name })
        .at(0);
      expect(toggle?.disabled).toBe(true);
    }

    answer();
    await waitFor(() => {
      expect(path).toBe(`/sessions/${SESSION_ID}`);
    });
  });

  it("does not navigate when the form that started the launch is gone", async () => {
    const answer = deferredLaunch();

    const { client: queries, closeDrawer } = mount();
    const form = await openForm();
    fireEvent.click(
      within(form).getByRole("button", { name: "Open in session" }),
    );
    await waitFor(() => {
      expect(createSession).toHaveBeenCalledTimes(1);
    });

    // What Escape does to the drawer: the panel goes, the request does not.
    closeDrawer();
    const invalidate = vi.spyOn(queries, "invalidateQueries");
    answer();

    // The session was still created, so the caches are still settled — but the
    // user stays where they went.
    await waitFor(() => {
      expect(keysPassedTo(invalidate)).toContainEqual([
        "projects",
        PROJECT_ID,
        "sessions",
      ]);
    });
    expect(path).toBe(`/projects/${PROJECT_ID}`);
    expect(screen.getByText("the board")).toBeDefined();
  });
});

/** The query keys one `invalidateQueries` spy was asked for. */
function keysPassedTo(spy: {
  mock: { calls: [{ queryKey?: unknown }?, ...unknown[]][] };
}): unknown[] {
  return spy.mock.calls.map((call) => call[0]?.queryKey);
}
