---
id: ncv5g
title: Scaffold the frontend with Vite, React 19, TypeScript strict, Tailwind 4, ESLint and the dev proxy
status: done
priority: P0
created: "2026-09-16T20:25:50.512172037Z"
updated: "2026-09-17T05:17:15.492896328Z"
tags:
  - frontend
  - infra
parent: sywed
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Create `frontend/` as a buildable Vite + React 19 + TypeScript (strict) application with Tailwind CSS 4, ESLint, the runtime libraries the frontend epics need already installed, the `SPEC.md` "Frontend" directory structure in place, and a dev server that proxies `/api` and `/ws` to the orchestrator. It renders a single placeholder route; pages, services and stores are delivered by the frontend epics.

## Documents
- `SPEC.md` "Frontend": stack list and the `frontend/src/` structure (`components/`, `pages/`, `session/`, `tasks/`, `services/`, `hooks/`, `types/`, `utils/`).
- `ARCHITECTURE.md` "Frontend architecture": Vite-built React 19 + TypeScript served by nginx; `/api` and `/ws` are the two proxied prefixes.
- `CLAUDE.md` "Frontend conventions" (stack, named exports, functional components) and "Code quality" (`npm run lint && npx tsc -b && npm run build && npm run test:e2e`).
- `README.md` "Development", "Frontend": `npm install`, `npm run dev  # proxies /api and /ws to the orchestrator`.

## Acceptance criteria
- [ ] `frontend/package.json` has scripts `dev`, `build` (`tsc -b && vite build`), `lint` (`eslint .`), `preview`, `test:unit` (`vitest run`); `test:e2e` is added by the Playwright task.
- [ ] Vitest is installed as the unit-test runner: dev dependencies `vitest` and `jsdom`, a `test` block in `vite.config.ts` with `environment: "jsdom"`, test files matching `src/**/*.test.ts(x)` beside the module they test, linted by ESLint and excluded from the production build. One trivial `src/utils/example.test.ts` proves the runner works; real tests arrive with the stores and services that need them (Frontend foundation, project and session views, and task board epics).
- [ ] Dependencies installed with exact stack versions: `react@19`, `react-dom@19`, `react-router@7` (package `react-router`, not `react-router-dom`), `@tanstack/react-query`, `zustand`, `@tanstack/react-virtual`, `react-markdown`, `@xterm/xterm` (plus `@xterm/addon-fit`), `@heroicons/react`; dev: `vite`, `@vitejs/plugin-react`, `typescript`, `tailwindcss@4` with `@tailwindcss/vite`, `eslint` with `typescript-eslint`, `eslint-plugin-react-hooks`, `eslint-plugin-react-refresh`.
- [ ] `tsconfig.app.json` has `"strict": true`, `"noUnusedLocals": true`, `"noUnusedParameters": true`, `"noFallthroughCasesInSwitch": true`, `"verbatimModuleSyntax": true`; `tsconfig.json` uses project references so `npx tsc -b` type-checks both app and node configs.
- [ ] `vite.config.ts` registers `@vitejs/plugin-react` and `@tailwindcss/vite`, and sets `server.proxy` to `{ "/api": { target: "http://localhost:7000", changeOrigin: true }, "/ws": { target: "ws://localhost:7000", ws: true } }` with the port overridable through `VITE_API_TARGET` (default `http://localhost:7000`, matching `API_PORT` default 7000).
- [ ] `src/index.css` contains `@import "tailwindcss";` and the operator-console base tokens (dark background, monospace font stack variable) so later UI work extends rather than replaces it.
- [ ] `src/main.tsx` mounts `<App />` inside `QueryClientProvider` and a `BrowserRouter`; `src/App.tsx` defines the route table with one placeholder route `/` rendering a `HealthPlaceholderPage` (named export) that reads `GET /api/health` through a temporary `services/health.ts` using plain `fetch` **inside services/** (components never call `fetch`).
- [ ] Every structure directory exists: `src/components/`, `src/pages/`, `src/session/`, `src/tasks/`, `src/services/`, `src/hooks/`, `src/types/`, `src/utils/`, each with an `index.ts` barrel (may be empty exports) so imports resolve and `noUnusedLocals` is not tripped.
- [ ] `frontend/.gitignore` covers `node_modules/`, `dist/`, `test-results/`, `playwright-report/` (root `.gitignore` already has `node_modules/` and `dist/`).
- [ ] `cd frontend && npm ci && npm run lint && npx tsc -b && npm run build && npm run test:unit` pass; `npm run dev` serves the placeholder and proxies `/api/health`.

## Implementation notes
- Bootstrap with `npm create vite@latest frontend -- --template react-ts`, then add Tailwind 4 through the Vite plugin (no `tailwind.config.js`, no PostCSS config; Tailwind 4 is CSS-first).
- ESLint flat config (`eslint.config.js`) extending `typescript-eslint` recommended-type-checked is acceptable but must keep `npm run lint` under a few seconds; `react-hooks/rules-of-hooks` and `react-hooks/exhaustive-deps` must be errors.
- Named exports only; no default export except where Vite requires it (`vite.config.ts`, `eslint.config.js`).
- Keep `src/services/health.ts` deliberately tiny; the Frontend foundation epic replaces it with `apiClient.ts`.
- Do not create `AuthLayout`, `PageLayout`, `apiClient`, stores or pages here; they belong to the frontend epics. Do not invoke the `/frontend-design` skill for the placeholder page; it applies when real UI is shaped.
- Commit `package-lock.json`.

## Edge cases
- `verbatimModuleSyntax` requires `import type` for type-only imports; the placeholder must compile under it so later contributors see the pattern.
- The `ws: true` proxy entry is required for the session WebSocket (`/ws/sessions/{id}`) to work through the dev server.
- `npm run build` must not emit chunk-size warnings as errors; leave Vite defaults.

## Testing
- The acceptance test is the command chain `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`; Vitest is the unit runner, Playwright the end-to-end runner.
- Manually verify with the orchestrator skeleton running that `http://localhost:5173/` shows the health JSON via the proxy.

## Documentation
- `CLAUDE.md`: add Vitest to the "Frontend conventions" stack line, append `npm run test:unit` to the frontend chain under "Code quality", and add "Frontend unit tests use Vitest; test files sit beside the module as `*.test.ts`" under "Testing expectations". `README.md` "CI", Frontend CI row: `lint, typecheck, unit tests, build`. Same commit. If `README.md` needs the `VITE_API_TARGET` mention, add one sentence under "Frontend" too.

## Assumes from other epics
- none.