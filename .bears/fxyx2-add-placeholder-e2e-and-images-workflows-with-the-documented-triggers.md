---
id: fxyx2
title: Add placeholder E2E and Images workflows with the documented triggers
status: open
priority: P2
created: "2026-09-16T20:29:46.468172730Z"
updated: "2026-09-16T20:29:46.468172730Z"
tags:
  - infra
  - tests
depends_on:
  - bpwq5
  - xxufa
parent: sywed
---

## Summary
Add `.github/workflows/e2e.yml` and `.github/workflows/images.yml` with the triggers and job names the `README.md` "CI" table specifies, so the End-to-end tests and Session container images epics only fill in steps rather than negotiate workflow structure. The E2E placeholder runs the Playwright smoke test against the Vite dev server (no orchestrator yet); the Images placeholder builds nothing yet but checks out and lists `images/` so the workflow is registered and green.

## Documents
- `README.md` "CI": rows `E2E | orchestrator/** or frontend/** | Playwright against a real orchestrator, Postgres and the stub session image` and `Images | images/** | Build session images; smoke-run the entrypoint`.
- `CLAUDE.md` "Testing expectations", Frontend E2E and Engine tests (CI runs engine tests on both Podman and Docker: the Container engine epic's concern, not these placeholders).

## Acceptance criteria
- [ ] `.github/workflows/e2e.yml` named `E2E`, triggers `push` (branches `main`) and `pull_request` with `paths: ["orchestrator/**", "frontend/**", "images/**", ".github/workflows/e2e.yml"]`, `concurrency` group `e2e-${{ github.ref }}`, `permissions: contents: read`; job `playwright` on `ubuntu-latest`: checkout, `actions/setup-node@v4` (node 22, npm cache on `frontend/package-lock.json`), `npm ci`, `npx playwright install --with-deps chromium`, `npm run test:e2e` with `CI=true` (config's `webServer` starts Vite), `actions/upload-artifact@v4` of `frontend/playwright-report` on failure. Commented, named step slots for the E2E epic in order: `# Session images epic: build images/stub` — `# End-to-end tests epic: start Postgres 18 service, build orchestrator with --features integration-tests, export PLAYWRIGHT_API_URL`.
- [ ] `.github/workflows/images.yml` named `Images`, triggers on `paths: ["images/**", ".github/workflows/images.yml"]` for `push` to `main` and `pull_request`; job `build` on `ubuntu-latest` with a `strategy.matrix.image: [claude, stub]` and steps: checkout, then a step that runs `test -d images/${{ matrix.image }} && docker build -t mars-session-${{ matrix.image }}:ci images/${{ matrix.image }} || echo "images/${{ matrix.image }} not present yet"` so the workflow is green before the images exist and builds them automatically once they do; a commented slot `# Session images epic: smoke-run the entrypoint`.
- [ ] Both workflows are syntactically valid (`actionlint` locally or the GitHub UI) and the E2E workflow is green on the scaffold smoke test.
- [ ] `frontend/playwright.config.ts` `retries` and `reuseExistingServer` behave as designed under `CI=true` (retries 1, fresh server).

## Implementation notes
- Files: `.github/workflows/e2e.yml`, `.github/workflows/images.yml`.
- Match the style of `orchestrator.yml` and `frontend.yml` (same action majors, `permissions`, `concurrency`, `timeout-minutes`).
- The `images` matrix uses `docker build` because `ubuntu-latest` ships Docker; the Session images epic may switch to `podman build` or add a Podman matrix leg.
- No secrets are referenced in either workflow.

## Edge cases
- `paths` filters: the E2E workflow must also trigger on `images/**` because the stub image is part of the E2E stack (README lists `orchestrator/**` or `frontend/**`; adding `images/**` is a superset and is noted in the README in the same commit).
- Uploading the Playwright report only on failure keeps artifacts small; `if: failure()` on the upload step.

## Testing
- Open a PR touching `frontend/**` and `images/**` (a `.gitkeep` under `images/` is acceptable if the directory does not exist) and confirm both workflows run and pass.

## Documentation
- `README.md` "CI": update the E2E row's trigger column to `orchestrator/**, frontend/** or images/**` in the same commit.

## Assumes from other epics
- End-to-end tests epic replaces the placeholder steps with the real orchestrator, Postgres and stub image stack.
- Session container images epic adds the entrypoint smoke run and the image directories.