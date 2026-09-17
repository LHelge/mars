---
id: xxufa
title: "Add the Frontend CI workflow: lint, typecheck and build on frontend/** changes"
status: done
priority: P1
created: "2026-09-16T20:27:46.649858538Z"
updated: "2026-09-17T05:21:04.462573322Z"
tags:
  - infra
  - frontend
depends_on:
  - ncv5g
parent: sywed
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Add `.github/workflows/frontend.yml`, the "Frontend CI" workflow from the `README.md` "CI" table: on pushes to `main` and pull requests that touch `frontend/**`, install with `npm ci`, then run lint, typecheck and build. It must be green on `main` once the frontend scaffold is merged.

## Documents
- `README.md` "CI": row `Frontend CI | frontend/** | lint, typecheck, build`.
- `CLAUDE.md` "Code quality" (frontend chain) and "Git workflow" (feature branches, PRs to `main`).

## Acceptance criteria
- [ ] `.github/workflows/frontend.yml` named `Frontend CI`, triggers `push` (branches `main`) and `pull_request`, both with `paths: ["frontend/**", ".github/workflows/frontend.yml"]`.
- [ ] `concurrency: { group: frontend-${{ github.ref }}, cancel-in-progress: true }`; `permissions: contents: read`.
- [ ] One job `check` on `ubuntu-latest`, `defaults.run.working-directory: frontend`, steps: `actions/checkout@v4`, `actions/setup-node@v4` with `node-version: 22` and `cache: npm` (`cache-dependency-path: frontend/package-lock.json`), `npm ci`, `npm run lint`, `npx tsc -b`, `npm run test:unit`, `npm run build`.
- [ ] The workflow runs to green on the scaffold; the run URL is linked in the PR description.
- [ ] `npm run test:e2e` is **not** run here (E2E workflow placeholder task).

## Implementation notes
- Files: `.github/workflows/frontend.yml`.
- Node major must match the `engines` field added to `frontend/package.json` in this task (`"engines": { "node": ">=22" }`) so local and CI versions agree.
- Use `npm ci` (lockfile must be committed by the scaffold task).

## Edge cases
- `paths` filters mean a PR that only changes docs skips the workflow; branch protection must therefore not require this check unconditionally. Note this in the PR.
- Keep the job under five minutes; no browser installation here.

## Testing
- Open a PR containing the workflow and confirm the check appears and passes; then confirm the `main` push run is green.
- Local chain: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements `README.md` "CI" as written.

## Assumes from other epics
- none.