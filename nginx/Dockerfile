# Mars frontend image (README.md, "Deployment shape"; ARCHITECTURE.md,
# "Components"): nginx serving the built React application and proxying /api
# and /ws to the orchestrator.
#
# Build (README.md, "Development" → "Deployment images") from the repository
# root, because the build context has to hold both `frontend/` and `nginx/`:
#   podman build -f nginx/Dockerfile -t mars-nginx:dev .
#
# `/.dockerignore` keeps `frontend/node_modules`, `frontend/dist` and the other
# build outputs out of that context. No BuildKit-only syntax is used (no
# `--mount=type=cache`), so `podman build` and `docker build` behave alike.

# ---------------------------------------------------------------------------
# Builder
# ---------------------------------------------------------------------------
FROM node:24-alpine AS build

WORKDIR /src

# Manifests first, so a source-only change reuses the install layer. `npm ci`
# fails loudly when package-lock.json has drifted from package.json, which is
# the intent. NODE_ENV is deliberately left unset here: with `NODE_ENV=production`
# npm would skip devDependencies and the build has no Vite or TypeScript. The
# production build is selected by the `build` script itself (`tsc -b && vite
# build`), not by the environment.
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci

# The rest of the frontend. Linting and Playwright are CI's job, not the
# image's; `npm run build` runs `tsc -b` and so still type-checks.
COPY frontend/ ./
RUN npm run build

# ---------------------------------------------------------------------------
# Runtime
# ---------------------------------------------------------------------------
# The minor is pinned rather than tracking `nginx:alpine` so a CI build is
# reproducible; bump this tag deliberately. The compose and CI definitions name
# the image only through this Dockerfile.
FROM nginx:1.27-alpine

LABEL org.opencontainers.image.title="mars-nginx" \
      org.opencontainers.image.description="Mars frontend: the built React application served by nginx, proxying /api and /ws" \
      org.opencontainers.image.source="https://github.com/LHelge/mars"

# http-level settings, and the server block as a template. The official image's
# entrypoint renders /etc/nginx/templates/*.template into /etc/nginx/conf.d/
# with envsubst before starting nginx, so no custom entrypoint is needed; the
# http block includes /etc/nginx/conf.d/*.conf.
COPY nginx/nginx.conf /etc/nginx/nginx.conf
COPY nginx/default.conf.template /etc/nginx/templates/default.conf.template

# Vite's output. No VITE_* variable is baked in: the application addresses the
# orchestrator through relative /api and /ws URLs, so one image serves every
# deployment and PUBLIC_URL stays the orchestrator's concern.
COPY --from=build /src/dist /usr/share/nginx/html

# Defaults matching README.md, "Configuration"; a template carries no defaults
# of its own. The filter restricts envsubst to exactly these two names, so
# nginx's own $host, $uri, $scheme and $http_upgrade survive even when
# variables of those names exist in the container environment.
ENV ORCHESTRATOR_HOST=orchestrator \
    API_PORT=7000 \
    NGINX_ENVSUBST_FILTER='^(ORCHESTRATOR_HOST|API_PORT)$'

# The official image runs as root and drops to the `nginx` user for its
# workers, which is why port 80 works; that is kept for v1. A rootless-friendly
# variant is a drop-in should one be wanted later:
# `nginxinc/nginx-unprivileged` listens on 8080 as an unprivileged user, which
# would change this EXPOSE, the `listen` in nginx/default.conf.template and the
# compose port mapping. Not done now.
EXPOSE 80

# ENTRYPOINT and CMD come from the base image: /docker-entrypoint.sh renders the
# templates and execs `nginx -g 'daemon off;'`. No HEALTHCHECK either, for the
# same reason as the orchestrator image: compose declares it, so interval and
# command live in one place.
