// The panel's refresh rule (`SPEC.md`, "Frontend", "Changes panel"): one
// fetch when it opens, and one more per `git` event — including a failed sync,
// which is exactly when a reviewer wants to see what the branch really holds.
// Nothing polls, so a `git` event that never arrives must not be simulated by
// a timer here either.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { getDiff } from "../services/git";
import { getProject, listBranches } from "../services/projects";
import type { Branch, Diff, Project, Session } from "../types";
import { ChangesPanel } from "./ChangesPanel";
import { disposeSessionStore, getSessionStore } from "./sessionStore";

vi.mock("../services/git", () => ({ getDiff: vi.fn() }));
vi.mock("../services/projects", () => ({
  getProject: vi.fn(),
  listBranches: vi.fn(),
}));

// Obviously fake fixture values (CLAUDE.md, rule 3).
const SESSION_ID = "00000000-0000-4000-8000-0000000000a1";
const PROJECT_ID = "00000000-0000-4000-8000-0000000000b2";

const DIFF: Diff = {
  base: "main",
  head: SESSION_ID,
  merge_base: "1111111111111111111111111111111111111111",
  files: [{ path: "src/one.ts", status: "M", additions: 1, deletions: 1 }],
  patch: `diff --git a/src/one.ts b/src/one.ts
--- a/src/one.ts
+++ b/src/one.ts
@@ -1,1 +1,1 @@
-const a = 1;
+const a = 2;
`,
  truncated: false,
};

const session = {
  id: SESSION_ID,
  project_id: PROJECT_ID,
  state: "running",
  branch: "sessions/fix-login",
} as Session;

function mount() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const invalidate = vi.spyOn(client, "invalidateQueries");
  const view = render(
    <QueryClientProvider client={client}>
      <ChangesPanel session={session} />
    </QueryClientProvider>,
  );
  return { client, invalidate, view };
}

/** A `git` event as the socket would deliver it. */
function gitEvent(seq: number, ok: boolean) {
  act(() => {
    getSessionStore(SESSION_ID).getState().applyEvent({
      kind: "git",
      seq,
      ts: "2026-03-01T12:00:00Z",
      op: "sync",
      ok,
      detail: { ref: `refs/sessions/${SESSION_ID}` },
    });
  });
}

beforeEach(() => {
  vi.mocked(getDiff).mockResolvedValue(DIFF);
  vi.mocked(getProject).mockResolvedValue({
    id: PROJECT_ID,
    default_branch: "main",
  } as Project);
  vi.mocked(listBranches).mockResolvedValue([
    { name: "main", kind: "head", commit: "1111111" },
  ] as Branch[]);
});

afterEach(() => {
  cleanup();
  disposeSessionStore(SESSION_ID);
  vi.clearAllMocks();
});

describe("ChangesPanel", () => {
  it("fetches the diff of the session branch when it opens", async () => {
    mount();

    await waitFor(() => {
      expect(screen.getAllByText("src/one.ts").length).toBeGreaterThan(0);
    });
    expect(vi.mocked(getDiff)).toHaveBeenCalledWith(
      PROJECT_ID,
      { head: SESSION_ID },
      undefined,
      // TanStack's cancellation signal: a diff the panel no longer wants is
      // abandoned rather than downloaded.
      expect.any(AbortSignal),
    );
    // The merge base identifies what the patch is measured from.
    expect(screen.getByText(/1111111 →/)).toBeDefined();
  });

  it("invalidates the diff query on a git event, and not before", async () => {
    const { invalidate } = mount();

    await waitFor(() => {
      expect(screen.getAllByText("src/one.ts").length).toBeGreaterThan(0);
    });
    expect(invalidate).not.toHaveBeenCalled();

    gitEvent(7, true);

    await waitFor(() => {
      expect(invalidate).toHaveBeenCalledWith({
        queryKey: [
          "projects",
          PROJECT_ID,
          "git",
          "diff",
          { head: SESSION_ID, base: null },
        ],
      });
    });
  });

  it("refetches after a failed sync too", async () => {
    const { invalidate } = mount();

    await waitFor(() => {
      expect(screen.getAllByText("src/one.ts").length).toBeGreaterThan(0);
    });
    gitEvent(8, false);

    await waitFor(() => {
      expect(invalidate).toHaveBeenCalledTimes(1);
    });
  });

  it("renders the patch of each file as diff lines", async () => {
    mount();

    await waitFor(() => {
      expect(screen.getByText("const a = 2;")).toBeDefined();
    });
    expect(screen.getByText("@@ -1,1 +1,1 @@")).toBeDefined();
  });
});
