---
id: uhp3q
title: "Write nginx/nginx.conf: static frontend, /api and /ws proxying, unbuffered task SSE and query-free access logs"
status: done
priority: P1
created: "2026-09-16T20:41:58.174822778Z"
updated: "2026-09-19T15:53:02.567534251Z"
tags:
  - infra
  - frontend
  - realtime
depends_on:
  - s52qg
  - "2f5u2"
parent: "5czwa"
attempts: 1
---

## Summary
Author the nginx server configuration that fronts Mars: it serves the built frontend with `index.html` fallback for client routes, proxies `/api/` and `/ws/` to the orchestrator API listener with HTTP/1.1, upgrades WebSockets on `/ws/` with a one-hour read timeout, serves the task SSE stream unbuffered, and logs the two token-carrying locations with a format that contains neither `$request` nor `$args`. The MCP listener (`MCP_PORT`, 7001) is never referenced. The configuration is a template so the upstream host and port follow `API_PORT`.

## Documents
- `ARCHITECTURE.md` "Frontend architecture", "nginx configuration requirements" (all five bullets: `/api/` with `proxy_http_version 1.1`; `/ws/` with `Upgrade`/`Connection` and `proxy_read_timeout` ≥ 1 h, pings every 30 s; `/api/projects/` paths ending in `/tasks/stream` with `proxy_buffering off`, `proxy_cache off`, `Connection ''`; log format without `$request`/`$args` or `access_log off` on both; everything else serves `index.html`).
- `ARCHITECTURE.md` "Components" (nginx: "Does not proxy the MCP listener. Configured for WebSocket upgrade and unbuffered SSE."), "Networks" (MCP listener reachable only through `mars-sessions` "because nginx never forwards to it").
- `SPEC.md` "Authentication" (WebSocket and SSE accept `?token=`; "nginx must not log query strings for those locations"; cookies `Path=/`, `Secure` when `PUBLIC_URL` is https), "WebSocket: session stream" (`GET /ws/sessions/{id}?after=&token=`), "SSE: task stream" (`GET /api/projects/{pid}/tasks/stream?token=`, `: keepalive` every 15 s, "nginx must serve this location with buffering off"), "REST API" (all routes under `/api`), "Frontend" routes list (client-side routes that must fall back to `index.html`).
- `README.md` "Deployment shape" (nginx serves the frontend and proxies `/api` and `/ws`; MCP endpoint not proxied), "Configuration" (`API_PORT` default 7000, `PUBLIC_URL`).

## Acceptance criteria
- [ ] File `nginx/default.conf.template` (consumed by the official image's `envsubst` template mechanism; the nginx Dockerfile task copies it to `/etc/nginx/templates/`) with variables `${ORCHESTRATOR_HOST}` (default `orchestrator`) and `${API_PORT}` (default `7000`) only; every other value is literal.
- [ ] `log_format noquery '$remote_addr - $remote_user [$time_local] "$request_method $uri $server_protocol" $status $body_bytes_sent "$http_user_agent" $request_time';` is defined at `http` level (in `nginx/nginx.conf`, which replaces the image's `/etc/nginx/nginx.conf`, or in a `conf.d` snippet included before the server) and contains neither `$request` nor `$args` nor `$request_uri` nor `$query_string`. The default `main` format is used for other locations.
- [ ] `location /ws/ { proxy_pass http://${ORCHESTRATOR_HOST}:${API_PORT}; proxy_http_version 1.1; proxy_set_header Upgrade $http_upgrade; proxy_set_header Connection "upgrade"; proxy_read_timeout 3600s; proxy_send_timeout 3600s; access_log /var/log/nginx/access.log noquery; ... }` plus `Host`, `X-Forwarded-For`, `X-Forwarded-Proto`, `X-Real-IP` headers.
- [ ] `location ~ ^/api/projects/[^/]+/tasks/stream$ { proxy_pass ...; proxy_http_version 1.1; proxy_set_header Connection ''; proxy_buffering off; proxy_cache off; chunked_transfer_encoding off; proxy_read_timeout 3600s; access_log /var/log/nginx/access.log noquery; }` — a regex location so it wins over the `/api/` prefix location (nginx evaluates regex locations before non-`^~` prefix matches; do not mark `/api/` as `^~`).
- [ ] `location /api/ { proxy_pass ...; proxy_http_version 1.1; proxy_set_header Connection ""; proxy_set_header Host $host; X-Forwarded-*; proxy_read_timeout 120s; client_max_body_size 10m; }` (the default `main` log format is fine here: REST calls carry the token in the header, not the query).
- [ ] `location / { root /usr/share/nginx/html; try_files $uri $uri/ /index.html; }` with `index.html` served with `Cache-Control: no-cache` and hashed Vite assets under `/assets/` with `Cache-Control: public, max-age=31536000, immutable`; `gzip on` for text, JS, CSS, JSON, SVG.
- [ ] No `location` mentions port 7001, `/mcp` or `MCP_PORT`; `grep -n '7001\|/mcp' nginx/` returns nothing.
- [ ] `server_tokens off;` and `listen 80;` only (TLS termination is outside nginx in v1; see notes).
- [ ] Verified with a throwaway container: with a stub upstream (e.g. `python3 -m http.server` or a tiny `socat` echo on port 7000) requests to `/ws/sessions/x?token=SECRET` and `/api/projects/p/tasks/stream?token=SECRET` produce access-log lines without `SECRET` or `?`, while `/api/health` logs normally; `/projects/abc` returns `index.html` with 200; `nginx -t` passes.

## Implementation notes
- Files: `nginx/nginx.conf` (http-level: `log_format noquery`, `map $http_upgrade $connection_upgrade { default upgrade; '' close; }` for the WebSocket `Connection` header, `include /etc/nginx/conf.d/*.conf;`), `nginx/default.conf.template` (the `server` block). Keep both under `nginx/` as `README.md` "Development" lists (`nginx/ nginx.conf and Dockerfile for the frontend image`).
- Use the `map`-based `Connection $connection_upgrade` on `/ws/` rather than a literal `"upgrade"` so non-upgrade requests on `/ws/` are not broken.
- The SSE location needs `proxy_set_header Connection ''` (empty) plus `proxy_http_version 1.1` so the upstream keeps the response open; `proxy_buffering off` alone is not enough with HTTP/1.0 upstream connections.
- `X-Forwarded-Proto` matters for the `Secure` cookie only insofar as the orchestrator derives it from `PUBLIC_URL` (SPEC "Authentication"); pass it anyway so a future change can use it.
- TLS: `README.md` "Deployment shape" draws `https` between browser and nginx but nothing specifies certificates. v1 decision for this task: nginx listens on plain 80 inside the compose network and the operator terminates TLS in front of it (host reverse proxy, Caddy, a cloud load balancer) or replaces this template. The compose task documents this in `README.md` "Running it"; this task only leaves a comment at the top of the template. Do not add a self-signed certificate.
- Log destination: keep the image default (`/var/log/nginx/access.log` symlinked to stdout in the official image) so `compose logs nginx` shows the lines; the acceptance test greps those.

## Edge cases
- `/ws/` requests without an `Upgrade` header (a plain GET by a scanner) must get the orchestrator's normal 4xx, not a 502 from nginx; the `map` handles this.
- Very long SSE connections: `proxy_read_timeout 3600s` exceeds the 15 s keepalive comfortably; a token revocation closes the stream from the orchestrator side (SPEC "Authentication"), nginx just passes the close through.
- `try_files` must not swallow `/api/` or `/ws/` 404s; the prefix locations take precedence over `/` so an unknown `/api/x` reaches the orchestrator and returns its `{status, error}` body.
- Large task descriptions and hand-off comments are JSON bodies well under 10 MB; `client_max_body_size 10m` is generous but bounded.
- `$uri` in `noquery` is the normalised path (no query string, decoded); that is what the acceptance criterion wants.

## Testing
- `podman run --rm -v ./nginx/nginx.conf:/etc/nginx/nginx.conf:ro -v ./nginx/default.conf.template:/etc/nginx/templates/default.conf.template:ro -e ORCHESTRATOR_HOST=127.0.0.1 -e API_PORT=7000 nginx:1.27-alpine nginx -t` passes.
- The manual log-hygiene check described in the acceptance criteria, recorded in the PR with the exact log lines (tokens obviously fake).
- No orchestrator or frontend code changes; no cargo/npm chains are required, but the file must be linted by `nginx -t` in the CI task of this epic.

## Documentation
- none: implements `ARCHITECTURE.md` "Frontend architecture" nginx requirements as written. The TLS-termination sentence lands with the compose task's README change.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": the API listener binds `0.0.0.0:API_PORT` and includes the `/api/health` route.
- "Real-time delivery": `/ws/sessions/{id}` and `/api/projects/{pid}/tasks/stream` exist; the paths above are taken from `SPEC.md` so this task can be done before them.