---
id: h7479
title: Write nginx/Dockerfile building the frontend and serving it from the nginx image with the config template
status: open
priority: P1
created: "2026-09-16T20:42:28.761293207Z"
updated: "2026-09-16T20:42:28.761293207Z"
tags:
  - infra
  - frontend
depends_on:
  - uhp3q
parent: "5czwa"
---

## Summary
Produce the frontend/nginx image: a multi-stage `nginx/Dockerfile` whose first stage runs `npm ci && npm run build` in `frontend/` and whose runtime stage is the official `nginx` Alpine image with the built `dist/` at `/usr/share/nginx/html`, the http-level `nginx.conf` and the `default.conf.template` from the previous task installed so the image's entrypoint renders the template from `ORCHESTRATOR_HOST` and `API_PORT`. The compose file builds this image for the `nginx` service.

## Documents
- `README.md` "Deployment shape" (nginx: "serves the built React frontend and proxies `/api` and `/ws`"), "Development" layout (`nginx/ nginx.conf and Dockerfile for the frontend image`), "Configuration" (`API_PORT`).
- `ARCHITECTURE.md` "Components" (nginx row), "Frontend architecture" (Vite-built React 19 + TypeScript "served by nginx").
- `CLAUDE.md` "Frontend conventions" (build is `tsc -b && vite build`; `npm run lint && npx tsc -b && npm run build` must pass).

## Acceptance criteria
- [ ] `nginx/Dockerfile` builder stage `FROM node:22-alpine AS build`, `WORKDIR /src`, copies `frontend/package.json` and `frontend/package-lock.json` first and runs `npm ci` (cached layer), then copies the rest of `frontend/` and runs `npm run build`; `NODE_ENV` is left unset for `npm ci` so devDependencies (Vite, TypeScript) install, and Vite's production build is selected by the `build` script itself.
- [ ] Runtime stage `FROM nginx:1.27-alpine` (pin the minor; comment that it is bumped deliberately), `COPY nginx/nginx.conf /etc/nginx/nginx.conf`, `COPY nginx/default.conf.template /etc/nginx/templates/default.conf.template`, `COPY --from=build /src/dist /usr/share/nginx/html`, `ENV ORCHESTRATOR_HOST=orchestrator API_PORT=7000`, `EXPOSE 80`. The official image's entrypoint renders `/etc/nginx/templates/*.template` into `/etc/nginx/conf.d/` with `envsubst`; no custom entrypoint.
- [ ] The build context is the **repository root** (the Dockerfile needs both `frontend/` and `nginx/`): `podman build -f nginx/Dockerfile -t mars-nginx:dev .` and the same with `docker build` succeed from a clean checkout. A root `.dockerignore` excludes `orchestrator/target`, `frontend/node_modules`, `frontend/dist`, `frontend/test-results`, `frontend/playwright-report`, `.git`, `data/`, `.env*`, `.bears/`.
- [ ] Running the image with `-e ORCHESTRATOR_HOST=127.0.0.1 -e API_PORT=7000 -p 8080:80` serves `/` (200, the app shell) and `/projects/x` (200, `index.html`); `nginx -T` inside the container shows the rendered upstream `http://127.0.0.1:7000`; `/api/health` returns 502 when nothing listens (proves the proxy is wired) — record in the PR.
- [ ] Image size under 60 MB.
- [ ] `README.md` "Development" build line for this image (`podman build -f nginx/Dockerfile -t mars-nginx:dev .`) next to the orchestrator image build line; the layout tree keeps `nginx/ nginx.conf and Dockerfile for the frontend image`.

## Implementation notes
- Files: `nginx/Dockerfile`, `/.dockerignore` (repository root), `README.md`.
- Vite reads `VITE_*` variables at build time; none are needed for production (the app uses relative `/api` and `/ws` URLs). Do not bake `PUBLIC_URL` into the image.
- Do not run `npm run lint` or Playwright in the Dockerfile; CI does that. Only `npm run build` (which runs `tsc -b`).
- Keep `nginx:1.27-alpine` rather than `nginx:alpine` so the CI build is reproducible; the compose task and the CI task reference this same tag only through the Dockerfile.
- The nginx image runs as root by default and drops to `nginx` for workers; keeping that is acceptable for v1 (port 80 inside the container). If a rootless-friendly variant is wanted later, `nginxinc/nginx-unprivileged` on port 8080 is the drop-in and would change `EXPOSE` and the compose port mapping; note this in a Dockerfile comment, do not do it now.

## Edge cases
- `npm ci` requires `package-lock.json` in sync with `package.json`; a drifted lockfile fails the build with a clear message, which is desired.
- The frontend build output must not include source maps in production unless `frontend/vite.config.ts` says so; leave Vite defaults.
- BuildKit vs Podman: avoid `--mount=type=cache` and other BuildKit-only syntax so `podman build` works without `buildah` extras.

## Testing
- Build on both engines from a scratch clone; run the serve smoke checks in the acceptance criteria; `podman run --rm mars-nginx:dev nginx -t`.
- `cd frontend && npm run lint && npx tsc -b && npm run build` still passes on the host (the Dockerfile does not change the frontend).

## Documentation
- `README.md` "Development" (one build line), same commit.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `frontend/` with `npm run build` = `tsc -b && vite build` and a committed `package-lock.json`.
- "Frontend foundation" and later frontend epics only add pages; the image build is unaffected by their content.