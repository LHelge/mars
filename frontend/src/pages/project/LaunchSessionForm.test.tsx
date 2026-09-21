// What the project page's launch form sends, and what it leaves to the server
// (`SPEC.md`, "Sessions": with no `base_ref` the server starts from the task's
// hand-off commit or the project's default branch, decided when the request
// arrives).
//
// The base starts empty and stays empty unless the user picks one. That is not
// a detail of the form: the default branch is editable on the same page, and a
// form that had copied it at render time would keep posting the old name — and
// would show it as a custom ref, because a branch that no longer exists is not
// in the list the select offers.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { useEffect } from "react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { listProfiles } from "../../services/profiles";
import { listBranches } from "../../services/projects";
import { getAgentCredentials } from "../../services/secrets";
import { createSession } from "../../services/sessions";
import { useTaskStore } from "../../tasks/taskStore";
import type {
  Profile,
  Project,
  Session,
  SessionCreateInput,
} from "../../types";
import { LaunchSessionForm } from "./LaunchSessionForm";

vi.mock("../../services/profiles", () => ({ listProfiles: vi.fn() }));
vi.mock("../../services/projects", () => ({ listBranches: vi.fn() }));
vi.mock("../../services/secrets", () => ({ getAgentCredentials: vi.fn() }));
vi.mock("../../services/sessions", () => ({ createSession: vi.fn() }));

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "00000000-0000-4000-8000-0000000000a1";
const PROFILE_ID = "00000000-0000-4000-8000-0000000000c3";
const SESSION_ID = "00000000-0000-4000-8000-0000000000d4";

function project(defaultBranch: string): Project {
  return {
    id: PROJECT_ID,
    name: "mars",
    remote_url: "git@example.invalid:acme/mars.git",
    default_branch: defaultBranch,
    status: "ready",
    status_message: null,
    last_fetched_at: null,
    max_attempts: 3,
    created_at: "2026-01-01T00:00:00Z",
    has_credential: true,
  };
}

function profile(): Profile {
  return {
    id: PROFILE_ID,
    project_id: PROJECT_ID,
    name: "implementer",
    kind: "conversational",
    backend: "claude",
    image: "ghcr.io/example/mars-session:fake",
    model: null,
    system_prompt: "",
    serves_states: ["ready"],
    secrets: [],
    mcp_tools: [],
    runtime: null,
    permission_mode: "bypass",
    partial_messages: true,
    idle_timeout_secs: 900,
    is_default: true,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
}

function session(): Session {
  return {
    id: SESSION_ID,
    project_id: PROJECT_ID,
    profile_id: PROFILE_ID,
    kind: "conversational",
    created_by: null,
    title: null,
    task_id: null,
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

function mount(defaultBranch = "main") {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const tree = (branch: string) => (
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/projects/${PROJECT_ID}`]}>
        <Probe />
        <Routes>
          <Route
            path="/projects/:id"
            element={<LaunchSessionForm project={project(branch)} />}
          />
          <Route path="/sessions/:id" element={<p>the session</p>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
  const rendered = render(tree(defaultBranch));
  return {
    client,
    /** The settings form on the same page saved a different default branch. */
    renameDefaultBranch: (branch: string) => {
      rendered.rerender(tree(branch));
    },
  };
}

/** The form is ready once the profile list has arrived and one is selected. */
async function waitForProfiles(): Promise<void> {
  await screen.findByRole("option", { name: /implementer/ });
}

function baseRefSelect(): HTMLSelectElement {
  return screen.getByLabelText<HTMLSelectElement>("Base ref");
}

beforeEach(() => {
  path = "";
  useTaskStore.setState({ states: [] });
  vi.mocked(listProfiles).mockResolvedValue([profile()]);
  vi.mocked(listBranches).mockResolvedValue([
    { name: "main", kind: "head", commit: "a".repeat(40) },
    { name: "develop", kind: "head", commit: "b".repeat(40) },
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
  vi.mocked(createSession).mockResolvedValue(session());
});

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

describe("LaunchSessionForm", () => {
  it("follows the project's default branch instead of posting a copy of it", async () => {
    const { renameDefaultBranch } = mount("main");
    await waitForProfiles();

    // Nothing is chosen, and the empty entry says what the server would pick.
    expect(baseRefSelect().value).toBe("");
    expect(screen.getByText("Project default (main)")).toBeDefined();

    renameDefaultBranch("develop");

    // The field follows it, rather than holding `main` — which, once it is no
    // longer the default, is a ref like any other and would read as a custom
    // one.
    expect(baseRefSelect().value).toBe("");
    expect(screen.getByText("Project default (develop)")).toBeDefined();
    expect(screen.queryByLabelText("Custom base ref")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Launch session" }));

    await waitFor(() => {
      expect(createSession).toHaveBeenCalledTimes(1);
    });
    expect(vi.mocked(createSession).mock.calls[0]?.[1]).toEqual({
      profile_id: PROFILE_ID,
    } satisfies SessionCreateInput);
  });

  it("sends the base the user chose, and only then", async () => {
    mount("main");
    await waitForProfiles();

    fireEvent.change(baseRefSelect(), { target: { value: "develop" } });
    fireEvent.click(screen.getByRole("button", { name: "Launch session" }));

    await waitFor(() => {
      expect(createSession).toHaveBeenCalledTimes(1);
    });
    expect(vi.mocked(createSession).mock.calls[0]?.[1]).toEqual({
      profile_id: PROFILE_ID,
      base_ref: "develop",
    } satisfies SessionCreateInput);
  });

  it("settles the sessions list and the board, and goes to the session", async () => {
    const { client } = mount("main");
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const board = vi.spyOn(useTaskStore.getState(), "invalidate");
    await waitForProfiles();

    fireEvent.click(screen.getByRole("button", { name: "Launch session" }));

    await waitFor(() => {
      expect(
        invalidate.mock.calls.map((call) => call[0]?.queryKey),
      ).toContainEqual(["projects", PROJECT_ID, "sessions"]);
    });
    expect(board).toHaveBeenCalled();

    await waitFor(() => {
      expect(path).toBe(`/sessions/${SESSION_ID}`);
    });
  });
});
