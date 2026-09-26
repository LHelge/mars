// The startup path of `SPEC.md`, "Frontend", Rules, with a stored token whose
// session is gone: `GET /users/me` answers 401, the single refresh answers 401,
// and the sign-out handler this component registers clears the cache and lands
// on `/login`.
//
// The module graph is reset per test so `services/auth` re-reads the seeded
// `localStorage` token at import: only a real login installs a token any other
// way, and this test needs the "token but no user" state a reload produces.
// Everything is therefore imported dynamically and elements are built with
// `createElement`, so the tree never mixes two copies of React.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { User } from "./types";

const TOKEN_KEY = "mars.access_token";

// Obviously fake fixture values (CLAUDE.md, rule 3).
const user: User = {
  id: "00000000-0000-0000-0000-000000000001",
  username: "tester",
  email: "tester@example.invalid",
  admin: false,
  must_change_password: false,
  notify_email: true,
  created_at: "2026-01-01T00:00:00Z",
};

const TASK_PATH = "/projects/00000000-0000-4000-8000-0000000000a1/tasks/7";

/** An access token shaped like the orchestrator's, with a readable `sub`. */
function accessToken(sub: string): string {
  const payload = globalThis.btoa(JSON.stringify({ sub })).replace(/=+$/, "");
  return `fake-header.${payload}.not-a-signature`;
}

function unauthorized(): Response {
  return {
    ok: false,
    status: 401,
    statusText: "Unauthorized",
    text: () =>
      Promise.resolve(JSON.stringify({ status: 401, error: "unauthorized" })),
  } as unknown as Response;
}

let teardown: (() => void) | null = null;

afterEach(() => {
  teardown?.();
  teardown = null;
  vi.unstubAllGlobals();
  globalThis.localStorage.clear();
});

describe("AuthBootstrap", () => {
  it("signs out and returns to login when the stored session is gone", async () => {
    const requested: string[] = [];
    vi.stubGlobal("fetch", (input: RequestInfo | URL) => {
      // `apiClient` always passes a string URL; keep the assertion honest.
      if (typeof input === "string") {
        requested.push(input);
      }
      return Promise.resolve(unauthorized());
    });
    globalThis.localStorage.setItem(TOKEN_KEY, "fake-stale-access-token");

    vi.resetModules();
    const { createElement } = await import("react");
    const { render, screen, waitFor, cleanup } =
      await import("@testing-library/react");
    const { MemoryRouter, Route, Routes, useLocation } =
      await import("react-router");
    const { QueryClient, QueryClientProvider } =
      await import("@tanstack/react-query");
    const { AuthBootstrap } = await import("./AuthBootstrap");
    teardown = cleanup;

    function Probe() {
      const { pathname } = useLocation();
      return createElement("span", { "data-testid": "path" }, pathname);
    }

    const client = new QueryClient();
    client.setQueryData(["users", "list"], ["stale"]);

    render(
      createElement(
        QueryClientProvider,
        { client },
        createElement(
          MemoryRouter,
          { initialEntries: ["/"] },
          createElement(
            AuthBootstrap,
            null,
            createElement(
              Routes,
              null,
              createElement(Route, {
                path: "/",
                element: createElement(Probe),
              }),
              createElement(Route, {
                path: "/login",
                element: createElement(Probe),
              }),
            ),
          ),
        ),
      ),
    );

    await waitFor(() => {
      expect(screen.getByTestId("path").textContent).toBe("/login");
    });

    expect(requested).toEqual(["/api/users/me", "/api/auth/refresh"]);
    expect(globalThis.localStorage.getItem(TOKEN_KEY)).toBeNull();
    expect(client.getQueryData(["users", "list"])).toBeUndefined();
  });

  it("offers a retry instead of signing out when the orchestrator is unreachable", async () => {
    vi.stubGlobal("fetch", () =>
      Promise.reject(new TypeError("Failed to fetch")),
    );
    globalThis.localStorage.setItem(TOKEN_KEY, "fake-access-token");

    vi.resetModules();
    const { createElement } = await import("react");
    const { render, screen, waitFor, cleanup } =
      await import("@testing-library/react");
    const { MemoryRouter } = await import("react-router");
    const { QueryClient, QueryClientProvider } =
      await import("@tanstack/react-query");
    const { AuthBootstrap } = await import("./AuthBootstrap");
    teardown = cleanup;

    render(
      createElement(
        QueryClientProvider,
        { client: new QueryClient() },
        createElement(
          MemoryRouter,
          { initialEntries: ["/"] },
          createElement(
            AuthBootstrap,
            null,
            createElement("p", null, "application"),
          ),
        ),
      ),
    );

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(
        "orchestrator unreachable",
      );
    });
    expect(screen.getByRole("button", { name: "Retry" })).toBeDefined();
    // Only a 401 signs out; the token survives an outage.
    expect(globalThis.localStorage.getItem(TOKEN_KEY)).toBe(
      "fake-access-token",
    );
  });

  it("offers a retry instead of rendering without a user when the orchestrator answers 500", async () => {
    vi.stubGlobal("fetch", () =>
      Promise.resolve({
        ok: false,
        status: 500,
        statusText: "Internal Server Error",
        text: () =>
          Promise.resolve(
            JSON.stringify({ status: 500, error: "internal server error" }),
          ),
      } as unknown as Response),
    );
    globalThis.localStorage.setItem(TOKEN_KEY, "fake-access-token");

    vi.resetModules();
    const { createElement } = await import("react");
    const { render, screen, waitFor, cleanup } =
      await import("@testing-library/react");
    const { MemoryRouter } = await import("react-router");
    const { QueryClient, QueryClientProvider } =
      await import("@tanstack/react-query");
    const { AuthBootstrap } = await import("./AuthBootstrap");
    teardown = cleanup;

    render(
      createElement(
        QueryClientProvider,
        { client: new QueryClient() },
        createElement(
          MemoryRouter,
          { initialEntries: ["/"] },
          createElement(
            AuthBootstrap,
            null,
            createElement("p", null, "application"),
          ),
        ),
      ),
    );

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toBe(
        "orchestrator unreachable",
      );
    });
    expect(screen.queryByText("application")).toBeNull();
    expect(globalThis.localStorage.getItem(TOKEN_KEY)).toBe(
      "fake-access-token",
    );
  });
});

/**
 * Where a sign-out leaves the user (`SPEC.md`, "Frontend", Rules and "Copy
 * links"). An access token lives fifteen minutes, so a session expiring under
 * a reader is ordinary; sending them back to the top of the application
 * afterwards loses the task or session they had open.
 */
describe("AuthBootstrap sign-out destination", () => {
  /**
   * A signed-in application at `entry`, whose every route renders the current
   * path and the `from` the router carries.
   */
  async function mountAt(entry: string) {
    vi.resetModules();
    const { createElement } = await import("react");
    const { act, cleanup, render, screen } =
      await import("@testing-library/react");
    const { MemoryRouter, Route, Routes, useLocation } =
      await import("react-router");
    const { QueryClient, QueryClientProvider } =
      await import("@tanstack/react-query");
    const auth = await import("./services/auth");
    const { AuthBootstrap } = await import("./AuthBootstrap");
    teardown = cleanup;

    auth.installSession({ user, access_token: accessToken("user-1") });

    function Probe() {
      const location = useLocation();
      // Router state is untyped by construction; narrow it before use.
      const state: unknown = location.state;
      const from =
        typeof state === "object" &&
        state !== null &&
        "from" in state &&
        typeof state.from === "string"
          ? state.from
          : "none";
      return createElement(
        "span",
        { "data-testid": "probe" },
        `${location.pathname}${location.search}|${from}`,
      );
    }

    const probe = createElement(Probe);
    render(
      createElement(
        QueryClientProvider,
        { client: new QueryClient() },
        createElement(
          MemoryRouter,
          { initialEntries: [entry] },
          createElement(
            AuthBootstrap,
            null,
            createElement(
              Routes,
              null,
              createElement(Route, { path: "/login", element: probe }),
              createElement(Route, { path: "*", element: probe }),
            ),
          ),
        ),
      ),
    );

    return {
      at: () => screen.getByTestId("probe").textContent,
      signOut: (reason: "user" | "refresh_failed") => {
        act(() => {
          auth.signOut(reason);
        });
      },
    };
  }

  it("returns an expired session to where it was", async () => {
    const app = await mountAt(`${TASK_PATH}?state=review`);

    app.signOut("refresh_failed");

    expect(app.at()).toBe(`/login|${TASK_PATH}?state=review`);
  });

  it("carries nothing when the user signed out deliberately", async () => {
    const app = await mountAt(TASK_PATH);

    app.signOut("user");

    expect(app.at()).toBe("/login|none");
  });

  it("takes the folded transcripts with it, so the next login sees none", async () => {
    vi.resetModules();
    const { createElement } = await import("react");
    const { act, cleanup, render } = await import("@testing-library/react");
    const { MemoryRouter } = await import("react-router");
    const { QueryClient, QueryClientProvider } =
      await import("@tanstack/react-query");
    const auth = await import("./services/auth");
    const { getSessionStore, retainedSessionIds } =
      await import("./session/sessionStore");
    const { AuthBootstrap } = await import("./AuthBootstrap");
    teardown = cleanup;

    auth.installSession({ user, access_token: accessToken("user-1") });
    render(
      createElement(
        QueryClientProvider,
        { client: new QueryClient() },
        createElement(
          MemoryRouter,
          { initialEntries: ["/"] },
          createElement(
            AuthBootstrap,
            null,
            createElement("p", null, "application"),
          ),
        ),
      ),
    );

    // One session read by the user who is about to leave, holding an
    // unredacted transcript and a refused message of theirs (ADR 0027).
    const sessionId = "00000000-0000-4000-8000-0000000000a2";
    const store = getSessionStore(sessionId);
    store.getState().applyEvent({
      seq: 1,
      ts: "2026-01-01T00:00:00Z",
      kind: "text",
      text: "the first user's transcript",
    });
    store.getState().inputRejected("client-1", "session is not running");

    act(() => {
      auth.signOut("user");
    });

    expect(retainedSessionIds()).not.toContain(sessionId);
    // And what the next login opens for the same session starts empty.
    const fresh = getSessionStore(sessionId).getState();
    expect(fresh.order).toEqual([]);
    expect(fresh.lastSeq).toBe(0);
    expect(fresh.lastRejection).toBeNull();
    expect(fresh.status).toBe("connecting");
  });

  it("keeps an unsafe destination out of the router state", async () => {
    // `/login` itself would bounce the user straight back out again.
    const app = await mountAt("/login");

    app.signOut("refresh_failed");

    expect(app.at()).toBe("/login|none");
  });

  it("loads the user of an account another tab signed in", async () => {
    const other: User = {
      ...user,
      id: "00000000-0000-0000-0000-000000000002",
      username: "other",
    };
    const getMe = vi.fn(() => Promise.resolve(other));
    vi.resetModules();
    vi.doMock("./services/users", () => ({ getMe }));
    const { createElement } = await import("react");
    const { act, cleanup, render, screen, waitFor } =
      await import("@testing-library/react");
    const { MemoryRouter } = await import("react-router");
    const { QueryClient, QueryClientProvider } =
      await import("@tanstack/react-query");
    const auth = await import("./services/auth");
    const { AuthBootstrap } = await import("./AuthBootstrap");
    teardown = () => {
      cleanup();
      vi.doUnmock("./services/users");
    };

    auth.installSession({ user, access_token: accessToken("user-1") });
    render(
      createElement(
        QueryClientProvider,
        { client: new QueryClient() },
        createElement(
          MemoryRouter,
          { initialEntries: ["/"] },
          createElement(
            AuthBootstrap,
            null,
            createElement("p", null, "application"),
          ),
        ),
      ),
    );
    expect(screen.getByText("application")).toBeDefined();

    // The other tab wrote its own token; this one adopts it and is left
    // holding a token with nobody attached to it, which nothing else would
    // resolve — the bootstrap's own load ran on mount.
    const theirs = accessToken("user-2");
    act(() => {
      globalThis.localStorage.setItem(TOKEN_KEY, theirs);
      globalThis.dispatchEvent(
        new StorageEvent("storage", {
          key: TOKEN_KEY,
          newValue: theirs,
          storageArea: globalThis.localStorage,
        }),
      );
    });

    await waitFor(() => {
      expect(getMe).toHaveBeenCalledTimes(1);
    });
    await waitFor(() => {
      expect(auth.getCurrentUser()).toEqual(other);
    });
    expect(screen.getByText("application")).toBeDefined();
  });
});
