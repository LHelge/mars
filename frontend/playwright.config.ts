import { defineConfig, devices } from "@playwright/test";

// The frontend under test. Defaults to the Vite dev server; point it at the
// nginx container (or any other origin) with PLAYWRIGHT_BASE_URL.
const baseURL = process.env.PLAYWRIGHT_BASE_URL ?? "http://localhost:5173";

const isCI = Boolean(process.env.CI);

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  workers: 1,
  retries: isCI ? 1 : 0,
  reporter: isCI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: "npm run dev",
    url: baseURL,
    // CI controls the server lifetime; locally a developer's `npm run dev` is reused.
    reuseExistingServer: !isCI,
  },
});
