---
id: bpwq5
title: "Add the Playwright skeleton: config with workers 1, test-helpers module and a smoke test"
status: done
priority: P2
created: "2026-09-16T20:27:34.842349010Z"
updated: "2026-09-17T05:24:18.648342990Z"
tags:
  - frontend
  - tests
depends_on:
  - ncv5g
parent: sywed
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Install Playwright in `frontend/` with the configuration the E2E epic will extend: `workers: 1`, a `webServer` entry for the Vite dev server, a `tests/utils/test-helpers.ts` module with the helper signatures the scenarios need, and one smoke test that loads the application root. This makes `npm run test:e2e` a real command from the start and gives the E2E epic a fixed structure to fill in; scenarios that need a running orchestrator are out of scope here.

## Documents
- `CLAUDE.md` "Testing expectations", Frontend E2E: Playwright, `workers: 1`, real orchestrator with `--features integration-tests`, fresh users per test through `/api/test/users`, helpers in `tests/utils/test-helpers.ts`, stub session image.
- `CLAUDE.md` "Code quality": `npm run lint && npx tsc -b && npm run build && npm run test:e2e`.
- `SPEC.md` "Test-only routes": `POST /test/users` body `{username, email, password, admin?}` → `{user, access_token}` (201; sets the refresh cookie; `must_change_password` false).
- `README.md` "CI": the E2E workflow row.

## Acceptance criteria
- [ ] `@playwright/test` is a dev dependency; `npm run test:e2e` runs `playwright test`; `npx playwright install --with-deps chromium` is documented in `frontend/README.md` (a short file created here) or the root README "Development", "Frontend" section.
- [ ] `frontend/playwright.config.ts`: `testDir: "./tests"`, `workers: 1`, `fullyParallel: false`, `retries: 0` locally and `1` in CI (`process.env.CI`), `reporter: "list"` (plus `html` in CI), `use.baseURL` from `PLAYWRIGHT_BASE_URL` (default `http://localhost:5173`), `trace: "retain-on-failure"`, project `chromium` only, and `webServer: { command: "npm run dev", url: baseURL, reuseExistingServer: !process.env.CI }`.
- [ ] `frontend/tests/utils/test-helpers.ts` exports typed helper stubs with the final signatures: `createTestUser(request: APIRequestContext, opts?: { admin?: boolean }): Promise<{ username: string; email: string; password: string; access_token: string }>` (posts to `${apiBaseUrl}/api/test/users` with a unique `username` such as `e2e-<random>` and a fake email under `example.test`), `login(page: Page, username: string, password: string): Promise<void>` (navigates to `/login`, fills the form, waits for `/`), and `apiBaseUrl` read from `PLAYWRIGHT_API_URL` (default `http://localhost:7000`). Helpers that depend on pages that do not exist yet are implemented against the documented routes and marked `// implemented against SPEC.md "Frontend" routes; exercised by the E2E epic`.
- [ ] `frontend/tests/smoke.spec.ts`: one test `app shell loads` that opens `/` and asserts the document title and that the root element rendered; it must pass without an orchestrator (the placeholder page's failed `/api/health` fetch is rendered as an error state, not an unhandled rejection).
- [ ] `tsconfig` includes `tests/**` in type-checking (a `tsconfig.e2e.json` or the node project) so `npx tsc -b` catches helper type errors; ESLint lints `tests/**`.
- [ ] `test-results/` and `playwright-report/` are git-ignored.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e` pass locally with Chromium installed.

## Implementation notes
- Files: `frontend/playwright.config.ts`, `frontend/tests/smoke.spec.ts`, `frontend/tests/utils/test-helpers.ts`, `frontend/package.json` (script + dev dependency), `frontend/.gitignore`, `frontend/tsconfig.*.json`, optional `frontend/README.md` (three lines: install browsers, run e2e, env vars).
- Use `test.describe.configure({ mode: "serial" })` only in the E2E epic where scenario order matters; the skeleton stays plain.
- Keep the placeholder page's fetch error handling in the scaffold task's `HealthPlaceholderPage`: `useQuery` with `retry: false` and an `Alert`-like text `orchestrator unreachable` so the smoke test is deterministic.

## Edge cases
- `reuseExistingServer` must be false in CI so the workflow controls the server lifetime; locally a developer's running `npm run dev` is reused.
- `PLAYWRIGHT_BASE_URL` may point at the nginx container in the Deployment packaging epic's verification; the config must not assume the Vite port.

## Testing
- `npm run test:e2e` passes the smoke test with and without an orchestrator running.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e`.

## Documentation
- Add the browser-install and `PLAYWRIGHT_BASE_URL` / `PLAYWRIGHT_API_URL` lines to `README.md` "Development", "Frontend" in the same commit (new commands the README does not yet list).

## Assumes from other epics
- End-to-end tests epic writes every real scenario, wires the real orchestrator + Postgres + stub image into the E2E workflow and may reshape `test-helpers.ts`.
- Authentication epic delivers `POST /api/test/users` behind `integration-tests`.