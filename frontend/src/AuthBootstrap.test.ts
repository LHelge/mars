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

const TOKEN_KEY = "mars.access_token";

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
    const { render, screen, waitFor, cleanup } = await import(
      "@testing-library/react"
    );
    const { MemoryRouter, Route, Routes, useLocation } = await import(
      "react-router"
    );
    const { QueryClient, QueryClientProvider } = await import(
      "@tanstack/react-query"
    );
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
              createElement(Route, { path: "/", element: createElement(Probe) }),
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
    const { render, screen, waitFor, cleanup } = await import(
      "@testing-library/react"
    );
    const { MemoryRouter } = await import("react-router");
    const { QueryClient, QueryClientProvider } = await import(
      "@tanstack/react-query"
    );
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
    expect(globalThis.localStorage.getItem(TOKEN_KEY)).toBe("fake-access-token");
  });
});
