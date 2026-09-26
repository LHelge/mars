import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { defineConfig, devices } from "@playwright/test";

/**
 * `tests/e2e-stack.sh up` writes the running stack's connection facts to
 * `frontend/.e2e/env` as plain `KEY=VALUE` lines (README.md, "Development" →
 * "End-to-end tests"). They are copied into `process.env` only where the
 * variable is not already set, so an explicit one on the command line wins and
 * a stack that is not up changes nothing.
 */
function loadStackEnv(): void {
  const file = resolve(dirname(fileURLToPath(import.meta.url)), ".e2e", "env");
  let contents: string;
  try {
    contents = readFileSync(file, "utf8");
  } catch {
    return;
  }
  for (const line of contents.split("\n")) {
    const trimmed = line.trim();
    const separator = trimmed.indexOf("=");
    if (trimmed.startsWith("#") || separator <= 0) continue;
    const key = trimmed.slice(0, separator);
    process.env[key] ??= trimmed.slice(separator + 1);
  }
}

loadStackEnv();

// The frontend under test. Defaults to the Vite dev server; point it at the
// nginx container (or any other origin) with PLAYWRIGHT_BASE_URL.
const baseURL = process.env.PLAYWRIGHT_BASE_URL ?? "http://localhost:5173";

// The orchestrator the dev server proxies `/api` and `/ws` to; API_PORT
// defaults to 7000 (README.md, "Configuration").
const apiURL = process.env.PLAYWRIGHT_API_URL ?? "http://localhost:7000";

// A dev server this config starts listens on the port `baseURL` names, so a
// stack brought up on another port (E2E_BASE_URL) is served there too.
const devPort = new URL(baseURL).port || "5173";

const isCI = Boolean(process.env.CI);

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  workers: 1,
  retries: isCI ? 1 : 0,
  // The JSON report is what `tests/coverage-check.mjs --skips` reads after a
  // run, so it is written locally as well as in CI.
  reporter: [
    ["list"],
    ["json", { outputFile: "test-results/results.json" }],
    ...(isCI ? [["html", { open: "never" }] as const] : []),
  ],
  use: {
    baseURL,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: `npm run dev -- --port ${devPort} --strictPort`,
    url: baseURL,
    // A dev server this config starts proxies to the orchestrator the stack
    // actually runs, which is not port 7000 when E2E_API_PORT says otherwise.
    // Half the default session history page, so `older history loads on
    // scroll-up` fills a page with half the inputs; still above the stub's
    // three recorded turns (90 events), so no other scenario opens a session
    // with history left behind (`src/session/historyPageSize.ts`).
    env: { VITE_API_TARGET: apiURL, VITE_SESSION_HISTORY_PAGE_SIZE: "100" },
    // CI controls the server lifetime; locally a developer's `npm run dev` is reused.
    reuseExistingServer: !isCI,
  },
});
